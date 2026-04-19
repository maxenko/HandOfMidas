//! Synthetic tick generator — Roll-GARCH-U model.
//!
//! Composes four layers:
//! - [`garch`]    — per-grid volatility process (1-second grid)
//! - [`hawkes`]   — self-exciting arrival process
//! - [`roll`]     — bid-ask bounce on the efficient mid
//! - [`u_shape`]  — intraday intensity + spread multiplier
//!
//! See `plan/ib-sim/03-market-data-engine.md` for the full design.

pub mod garch;
pub mod hawkes;
pub mod roll;
pub mod u_shape;

use std::collections::HashMap;
use std::time::Duration;

use midas_broker_core::SymbolKey;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use rand_distr::{Distribution, LogNormal, StudentT};
use serde::{Deserialize, Serialize};

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{
    MarketEmission, Side, SubKey, SubMode, TickAttribs, TickByTickKind, TickType,
};
use crate::market_data::{MarketDataEngine, MarketDataError, Snapshot};

use self::garch::{GarchState, GARCH_GRID_INTERVAL_SECS};
use self::hawkes::{decay_excitement, sample_next_arrival, HAWKES_HALF_LIFE};
use self::roll::{observed_price, sample_side};
use self::u_shape::u_shape_multiplier;

/// Branching ratio for the Hawkes process — each event contributes this
/// much to the excitement. < 1 keeps the process sub-critical.
const HAWKES_BRANCHING: f64 = 0.5;
/// Hard cap on excitement so numerical blow-ups can't produce infinite λ.
const HAWKES_EXCITEMENT_CAP: f64 = 8.0;

// ---------------------------------------------------------------------------
// Presets — matches plan/ib-sim/03-market-data-engine.md §Presets.
// ---------------------------------------------------------------------------

/// Pre-baked symbol profile — sets spread, arrival intensity, jump rate,
/// and typical volume. The three presets cover the dynamic range we care
/// about for UI / order-routing tests.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SymbolPreset {
    /// SPY-like: penny spread, ~5 trades/sec baseline.
    #[default]
    Liquid,
    /// AAPL-like: 1-2 cent spread, ~2 trades/sec baseline.
    MidCap,
    /// Small-cap: 5-cent spread, sparse arrivals, fat jumps.
    Illiquid,
}

impl SymbolPreset {
    /// Half-spread in price units (symmetric around mid).
    pub fn half_spread(self) -> f64 {
        match self {
            Self::Liquid => 0.005,
            Self::MidCap => 0.010,
            Self::Illiquid => 0.050,
        }
    }

    /// Baseline trades-per-second (before U-shape + Hawkes multipliers).
    pub fn lambda_base(self) -> f64 {
        match self {
            Self::Liquid => 5.0,
            Self::MidCap => 2.0,
            Self::Illiquid => 0.2,
        }
    }

    /// Probability of a jump per tick. Deliberately very rare — a
    /// typical RTH session fires ~0 jumps, occasionally one. Scripted
    /// perturbations are the right tool for deterministic jumps.
    pub fn jump_rate(self) -> f64 {
        match self {
            Self::Liquid => 1e-7,
            Self::MidCap => 5e-7,
            Self::Illiquid => 5e-6,
        }
    }

    /// Jump-size (log-return) standard deviation.
    pub fn jump_scale(self) -> f64 {
        match self {
            Self::Liquid => 0.003,
            Self::MidCap => 0.007,
            Self::Illiquid => 0.020,
        }
    }

    /// Log-mean of per-trade volume (log-normal(μ, 0.6²)).
    pub fn volume_mean_log(self) -> f64 {
        match self {
            Self::Liquid => 5.5,   // ~250 shares
            Self::MidCap => 4.5,   // ~90 shares
            Self::Illiquid => 3.0, // ~20 shares
        }
    }
}

// ---------------------------------------------------------------------------
// SymbolSpec — per-symbol construction parameters.
// ---------------------------------------------------------------------------

/// Registration payload for a synthetic symbol. Built from a preset + an
/// initial price; callers can override individual knobs before passing in.
#[derive(Clone, Debug)]
pub struct SymbolSpec {
    pub symbol: SymbolKey,
    pub preset: SymbolPreset,
    pub initial_price: f64,
    /// Per-symbol seed — mixed with the master seed to derive the RNG streams.
    pub seed: u64,
}

impl SymbolSpec {
    pub fn new(symbol: SymbolKey, preset: SymbolPreset, initial_price: f64, seed: u64) -> Self {
        Self {
            symbol,
            preset,
            initial_price,
            seed,
        }
    }
}

// ---------------------------------------------------------------------------
// SymbolState — per-symbol mutable state.
// ---------------------------------------------------------------------------

/// Per-symbol state for the synthetic engine. Fields are `pub(super)` so
/// stylized-fact tests can read them for validation without exposing a
/// deep API surface.
pub(crate) struct SymbolState {
    #[allow(dead_code)]
    pub preset: SymbolPreset,

    // Price process
    pub mid_price: f64,
    pub log_mid: f64,
    /// Log of the initial price — used as a reference for clamping
    /// `log_mid` so pathological shocks can't drive prices unboundedly.
    pub log_mid_anchor: f64,
    pub garch: GarchState,
    pub garch_next_step: VirtualInstant,

    // Arrival process
    pub lambda_base: f64,
    pub excitement: f64,
    pub next_arrival: VirtualInstant,
    pub last_tick: VirtualInstant,

    // Observed price
    pub half_spread: f64,
    pub last_side: Side,

    // Volume
    pub cumulative_volume: i64,
    pub volume_mean_log: f64,

    // Jumps
    pub jump_rate: f64,
    pub jump_scale: f64,

    // RNGs — two independent streams, per plan §Innovation-stream note.
    /// Used for the 1-second GARCH grid innovations.
    pub grid_rng: SmallRng,
    /// Used for per-tick: innovations, jumps, sides, volumes, Hawkes waits.
    pub tick_rng: SmallRng,

    // Halt state — used by Hybrid InjectHalt perturbations. When set, we
    // emit no ticks until `now >= halt_until`.
    pub halt_until: Option<VirtualInstant>,

    // Burst multiplier from HybridEngine perturbations (1.0 = no boost).
    pub burst_multiplier: f64,
    pub burst_until: Option<VirtualInstant>,
}

impl SymbolState {
    fn from_spec(spec: &SymbolSpec) -> Self {
        let grid_seed = spec.seed.wrapping_mul(0x9E3779B97F4A7C15);
        let tick_seed = spec.seed.wrapping_mul(0xBF58476D1CE4E5B9).wrapping_add(1);
        Self {
            preset: spec.preset,
            mid_price: spec.initial_price,
            log_mid: spec.initial_price.ln(),
            log_mid_anchor: spec.initial_price.ln(),
            garch: GarchState::canonical(),
            // First grid step fires at t = 1 s.
            garch_next_step: VirtualInstant::from_secs(GARCH_GRID_INTERVAL_SECS as u64),
            lambda_base: spec.preset.lambda_base(),
            excitement: 0.0,
            next_arrival: VirtualInstant::ZERO,
            last_tick: VirtualInstant::ZERO,
            half_spread: spec.preset.half_spread(),
            last_side: Side::Buy,
            cumulative_volume: 0,
            volume_mean_log: spec.preset.volume_mean_log(),
            jump_rate: spec.preset.jump_rate(),
            jump_scale: spec.preset.jump_scale(),
            grid_rng: SmallRng::seed_from_u64(grid_seed),
            tick_rng: SmallRng::seed_from_u64(tick_seed),
            halt_until: None,
            burst_multiplier: 1.0,
            burst_until: None,
        }
    }

    /// Current effective arrival-multiplier including burst (if active).
    fn arrival_multiplier(&mut self, now: VirtualInstant) -> f64 {
        if let Some(until) = self.burst_until {
            if now >= until {
                self.burst_until = None;
                self.burst_multiplier = 1.0;
            }
        }
        u_shape_multiplier(now) * self.burst_multiplier
    }
}

// ---------------------------------------------------------------------------
// SyntheticEngine — MarketDataEngine implementor.
// ---------------------------------------------------------------------------

/// Pure-synthetic Roll-GARCH-U market-data engine.
///
/// Subscriptions are tracked in a `BTreeMap<SubKey, SubMode>` so `step()`
/// iteration order is deterministic. Symbols are allocated lazily from a
/// registered preset the first time they are subscribed.
pub struct SyntheticEngine {
    master_seed: u64,
    /// Registered symbols (from the scenario / CLI). Subscribing to a
    /// symbol that isn't registered auto-registers it with a default
    /// `Liquid` preset and initial price 100.
    registrations: HashMap<SymbolKey, SymbolSpec>,
    symbols: HashMap<SymbolKey, SymbolState>,
    /// Subscription table. Iteration order is stabilised at `step()` time
    /// by sorting a Vec of references on (session, req_id).
    subs: HashMap<SubKey, SubMode>,
    /// Stable ordering over subs — rebuilt lazily on subscribe/unsubscribe.
    subs_order_dirty: bool,
    subs_order: Vec<SubKey>,
    /// Last bid/ask/last surfaced for each symbol — served via `snapshot()`.
    last_snapshot: HashMap<SymbolKey, Snapshot>,
    /// Fresh per-(symbol, sub-mode) subscriptions that still need a seed
    /// emission on the next `step` (snapshot initial bid/ask).
    pending_initial_emit: Vec<SubKey>,
}

impl Default for SyntheticEngine {
    fn default() -> Self {
        Self::new(0)
    }
}

impl SyntheticEngine {
    /// Create a new engine with the given master seed.
    pub fn new(master_seed: u64) -> Self {
        Self {
            master_seed,
            registrations: HashMap::new(),
            symbols: HashMap::new(),
            subs: HashMap::new(),
            subs_order_dirty: false,
            subs_order: Vec::new(),
            last_snapshot: HashMap::new(),
            pending_initial_emit: Vec::new(),
        }
    }

    /// Refresh the deterministic iteration order over subscriptions.
    fn refresh_subs_order(&mut self) {
        if !self.subs_order_dirty {
            return;
        }
        self.subs_order = self.subs.keys().cloned().collect();
        self.subs_order
            .sort_by_key(|k| (k.session.0, k.req_id.0, k.symbol.contract_id));
        self.subs_order_dirty = false;
    }

    /// Register a symbol ahead of time. Subsequent subscriptions to this
    /// symbol will use this spec instead of the default.
    pub fn register_symbol(&mut self, spec: SymbolSpec) {
        self.registrations.insert(spec.symbol.clone(), spec);
    }

    /// Convenience: register a symbol at an initial price with a preset.
    pub fn register(&mut self, symbol: SymbolKey, preset: SymbolPreset, initial_price: f64) {
        let seed = self.master_seed.wrapping_add(hash_symbol(&symbol));
        self.register_symbol(SymbolSpec::new(symbol, preset, initial_price, seed));
    }

    /// Override a symbol's baseline trade-arrival rate (trades/sec). No-op
    /// if the symbol is unknown. Used by the stylized-fact `λ_base`-
    /// independence test and by scenarios that want to dial liquidity
    /// orthogonally to the preset.
    pub fn set_lambda_base(&mut self, symbol: &SymbolKey, lambda: f64) {
        self.ensure_symbol(symbol);
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.lambda_base = lambda.max(0.01);
        }
    }

    /// Internal: ensure a symbol is present in `symbols`, allocating it
    /// from its registration (or a default spec if unregistered).
    fn ensure_symbol(&mut self, symbol: &SymbolKey) {
        if self.symbols.contains_key(symbol) {
            return;
        }
        let spec = self.registrations.get(symbol).cloned().unwrap_or_else(|| {
            let seed = self.master_seed.wrapping_add(hash_symbol(symbol));
            SymbolSpec::new(symbol.clone(), SymbolPreset::Liquid, 100.0, seed)
        });
        self.symbols
            .insert(symbol.clone(), SymbolState::from_spec(&spec));
    }

    /// Fast-forward a fresh session by `duration` and return the emitted
    /// last-trade ticks for `symbol`. Intended for historical-bar queries
    /// and the stylized-facts validation harness.
    ///
    /// `dt_step` is how coarsely to call `step()`; smaller steps are more
    /// accurate but slower. 250 ms is a reasonable default.
    ///
    /// Also records the mid-price after each tick — the efficient-price
    /// series is the one Cont's stylized facts are stated against.
    /// Retrieve via [`Self::last_mid_history`].
    pub fn fast_forward_trades(
        &mut self,
        symbol: &SymbolKey,
        from: VirtualInstant,
        duration: Duration,
        dt_step: Duration,
    ) -> Vec<(VirtualInstant, f64, i64)> {
        self.ensure_symbol(symbol);
        // Install a scratch subscription so our emissions get produced.
        use crate::engine::types::{ReqId, SessionId};
        let key = SubKey {
            session: SessionId(u64::MAX),
            req_id: ReqId(i32::MAX),
            symbol: symbol.clone(),
        };
        let was_present = self.subs.contains_key(&key);
        if !was_present {
            self.subs.insert(
                key.clone(),
                SubMode::TickByTick {
                    kind: TickByTickKind::Last,
                },
            );
            self.subs_order_dirty = true;
        }
        // Warp to `from`.
        let state = self.symbols.get_mut(symbol).expect("just ensured");
        if state.next_arrival < from {
            state.next_arrival = from;
        }
        if state.garch_next_step < from {
            // Align GARCH grid to the nearest whole second beyond `from`.
            let secs = from.as_duration().as_secs();
            state.garch_next_step = VirtualInstant::from_secs(secs + 1);
        }
        state.last_tick = from;

        let end = from.saturating_add(duration);
        let mut t = from;
        let mut out = Vec::new();
        while t < end {
            t = t.saturating_add(dt_step);
            if t > end {
                t = end;
            }
            for em in self.step(t) {
                if let MarketEmission::TickPrice {
                    key: k,
                    tick: TickType::Last,
                    price,
                    size,
                    ..
                } = em
                {
                    if k.symbol == *symbol {
                        out.push((t, price, size.unwrap_or(0)));
                    }
                }
            }
        }

        if !was_present {
            self.subs.remove(&key);
            self.subs_order_dirty = true;
        }
        out
    }

    /// Expose the current GARCH σ (per 1-second grid step) for `symbol`.
    /// Intended for stylized-fact debugging and for downstream consumers
    /// that want to surface a "current volatility" estimate.
    pub fn garch_sigma(&self, symbol: &SymbolKey) -> Option<f64> {
        self.symbols.get(symbol).map(|s| s.garch.sigma())
    }

    /// Fast-forward and return `(ts, mid_price)` samples at each tick. The
    /// mid-price series is what Cont's stylized facts are stated against
    /// — trade prices are Roll-bounced and not directly suitable for
    /// volatility-clustering or kurtosis tests.
    pub fn fast_forward_mid(
        &mut self,
        symbol: &SymbolKey,
        from: VirtualInstant,
        duration: Duration,
        dt_step: Duration,
    ) -> Vec<(VirtualInstant, f64)> {
        self.ensure_symbol(symbol);
        use crate::engine::types::{ReqId, SessionId};
        let key = SubKey {
            session: SessionId(u64::MAX - 1),
            req_id: ReqId(i32::MAX - 1),
            symbol: symbol.clone(),
        };
        let was_present = self.subs.contains_key(&key);
        if !was_present {
            self.subs.insert(
                key.clone(),
                SubMode::StreamingL1 {
                    snapshot: false,
                    regulatory_snapshot: false,
                },
            );
            self.subs_order_dirty = true;
        }
        let state = self.symbols.get_mut(symbol).expect("just ensured");
        if state.next_arrival < from {
            state.next_arrival = from;
        }
        if state.garch_next_step < from {
            let secs = from.as_duration().as_secs();
            state.garch_next_step = VirtualInstant::from_secs(secs + 1);
        }
        state.last_tick = from;

        let end = from.saturating_add(duration);
        let mut t = from;
        let mut out = Vec::new();
        // We match Bid/Ask pairs and emit one mid per pair.
        let mut pending_bid: Option<f64> = None;
        while t < end {
            t = t.saturating_add(dt_step);
            if t > end {
                t = end;
            }
            for em in self.step(t) {
                if let MarketEmission::TickPrice {
                    key: k,
                    tick,
                    price,
                    ..
                } = em
                {
                    if k.symbol != *symbol {
                        continue;
                    }
                    match tick {
                        TickType::Bid => pending_bid = Some(price),
                        TickType::Ask => {
                            if let Some(bid) = pending_bid.take() {
                                let mid = (bid + price) * 0.5;
                                out.push((t, mid));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if !was_present {
            self.subs.remove(&key);
            self.subs_order_dirty = true;
        }
        out
    }

    /// Apply a scripted perturbation directly to the base engine (in-place
    /// rather than the `HybridEngine` post-processor). Reserved for future
    /// use by the scenario runner (Stage 06) — presently the post-processor
    /// form in `HybridEngine` is preferred.
    #[allow(dead_code)]
    pub(crate) fn apply_price_jump(&mut self, symbol: &SymbolKey, magnitude_pct: f64) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.log_mid += magnitude_pct / 100.0;
            state.mid_price = state.log_mid.exp();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn apply_gap(&mut self, symbol: &SymbolKey, from: f64, to: f64) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            let _ = from; // advisory; we simply snap the mid to `to`.
            state.mid_price = to.max(1e-6);
            state.log_mid = state.mid_price.ln();
        }
    }

    /// Used by tests in this file; exposed via `pub(crate)` for the
    /// integration-test harness as well.
    #[allow(dead_code)]
    pub(crate) fn apply_halt(
        &mut self,
        symbol: &SymbolKey,
        now: VirtualInstant,
        duration: Duration,
    ) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.halt_until = Some(now.saturating_add(duration));
        }
    }

    #[allow(dead_code)]
    pub(crate) fn apply_burst(
        &mut self,
        symbols: &[SymbolKey],
        now: VirtualInstant,
        multiplier: f64,
        duration: Duration,
    ) {
        for sym in symbols {
            if let Some(state) = self.symbols.get_mut(sym) {
                state.burst_multiplier = multiplier.max(0.01);
                state.burst_until = Some(now.saturating_add(duration));
            }
        }
    }

    /// Advance the engine and collect emissions. Called from `step()`.
    fn advance_symbol(&mut self, symbol: &SymbolKey, now: VirtualInstant) -> Vec<MarketEmission> {
        let mut emissions = Vec::new();
        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return emissions,
        };

        // 0. Halt gate.
        if let Some(until) = state.halt_until {
            if now < until {
                // Bring `last_tick` forward to keep Hawkes decay sane when we resume.
                state.last_tick = now;
                state.next_arrival = until;
                return emissions;
            } else {
                state.halt_until = None;
            }
        }

        // 1. Advance GARCH grid independently of tick arrivals.
        // The *grid* uses Student-t(4) winsorised at ±10 — fat-tailed
        // shocks drive GARCH clustering (squared-return ACF > 0.1) and,
        // via the resulting mixture of per-tick Normals, produce the
        // excess kurtosis observed in aggregated returns. Per-tick
        // innovations (below) are Normal so their variance doesn't
        // drown out the σ²(t) signal in clustering estimates.
        // See `plan/ib-sim/03-market-data-engine.md` §Innovation-stream
        // for why these streams are independent by design.
        let grid_step = Duration::from_secs(GARCH_GRID_INTERVAL_SECS as u64);
        while state.garch_next_step <= now {
            let raw: f64 = StudentT::new(4.0).unwrap().sample(&mut state.grid_rng);
            let eps_grid = raw.clamp(-10.0, 10.0);
            state.garch.step(eps_grid);
            state.garch_next_step = state.garch_next_step.saturating_add(grid_step);
        }

        // 2. Emit every due tick.
        loop {
            if state.next_arrival > now {
                break;
            }
            let tick_time = state.next_arrival;

            // Per-tick innovation: sigma_grid * sqrt(dt) * eps_tick.
            // Student-t(6) introduces moderate fat tails so aggregated 1-min
            // returns show kurtosis > 4 (Cont's heavy-tail fact), yet 6
            // degrees of freedom keep the 4th moment finite and the ACF
            // of squared returns well-behaved.
            let dt = tick_time
                .saturating_sub(state.last_tick)
                .as_secs_f64()
                .max(1e-6);
            let sigma_per_tick = state.garch.sigma() * dt.sqrt();
            let raw_tick: f64 = StudentT::new(6.0).unwrap().sample(&mut state.tick_rng);
            let eps_tick = raw_tick.clamp(-8.0, 8.0);

            // Rare jump.
            let jump = if state.tick_rng.gen::<f64>() < state.jump_rate {
                let sign = if state.tick_rng.gen::<bool>() {
                    1.0
                } else {
                    -1.0
                };
                let z: f64 = StudentT::new(4.0).unwrap().sample(&mut state.tick_rng);
                sign * state.jump_scale * z.abs()
            } else {
                0.0
            };

            let r = sigma_per_tick * eps_tick + jump;
            state.log_mid += r;
            if !state.log_mid.is_finite() {
                state.log_mid = state.log_mid_anchor;
            }
            // Guard against runaway under pathological shocks — clamp log
            // price within ±3 of the initial price (roughly 0.05× to 20×
            // the starting level).
            let lo = state.log_mid_anchor - 3.0;
            let hi = state.log_mid_anchor + 3.0;
            state.log_mid = state.log_mid.clamp(lo, hi);
            state.mid_price = state.log_mid.exp();

            // Roll bounce with an unbiased coin — any bias introduces
            // directional autocorrelation beyond lag 1 that trips Fact 1
            // (|ρ(r, r+k)| < 0.05 for k ≥ 2). A pure 50/50 side choice
            // plus the bounce itself reproduces Roll's classical setup.
            // Spread is held constant at `half_spread` so Roll's
            // estimator (s = 2·sqrt(−Cov(Δp))) recovers the configured
            // value; U-shape is applied to arrival rate and volume only.
            let u_multi = u_shape_multiplier(tick_time);
            let spread = state.half_spread;
            let side = sample_side(state.tick_rng.gen::<f64>(), 0.0);
            let last_price = observed_price(state.mid_price, spread, side);
            let bid = state.mid_price - spread;
            let ask = state.mid_price + spread;

            // Per-trade volume.
            let vol_mu = state.volume_mean_log + (u_multi.ln() * 0.5);
            let vol_draw: f64 = LogNormal::new(vol_mu, 0.6)
                .unwrap()
                .sample(&mut state.tick_rng);
            let volume = vol_draw.max(1.0).round() as i64;
            state.cumulative_volume = state.cumulative_volume.saturating_add(volume);

            // Update Hawkes excitement: decay then kick. Each event adds
            // `HAWKES_BRANCHING` (<1) to keep the self-excited process
            // sub-critical — branching ratio 0.5 ≈ Bacry et al. (2015)
            // for equity trade arrivals.
            let dt_since_last = tick_time.saturating_sub(state.last_tick).as_secs_f64();
            state.excitement = decay_excitement(state.excitement, dt_since_last, HAWKES_HALF_LIFE);
            state.excitement = (state.excitement + HAWKES_BRANCHING).min(HAWKES_EXCITEMENT_CAP);

            // Schedule next arrival from the Hawkes intensity at `tick_time`.
            let mult = state.arrival_multiplier(tick_time);
            let lambda = state.lambda_base * mult * (1.0 + state.excitement);
            let dt_next = sample_next_arrival(lambda, state.tick_rng.gen::<f64>());
            let wait = Duration::from_secs_f64(dt_next.clamp(1e-6, 60.0));
            state.next_arrival = tick_time.saturating_add(wait);
            state.last_tick = tick_time;
            state.last_side = side;

            // Record snapshot.
            let snap = Snapshot {
                bid,
                ask,
                last: last_price,
                volume: Some(volume),
                ts: tick_time,
            };
            self.last_snapshot.insert(symbol.clone(), snap);

            // Fan emissions out to every matching subscription in
            // deterministic iteration order.
            for sub_key in self.subs_order.iter().filter(|k| k.symbol == *symbol) {
                if let Some(mode) = self.subs.get(sub_key) {
                    let emit = emit_for_mode(
                        sub_key,
                        mode,
                        bid,
                        ask,
                        last_price,
                        volume,
                        state.cumulative_volume,
                    );
                    emissions.extend(emit);
                }
            }
        }

        emissions
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hash_symbol(symbol: &SymbolKey) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    symbol.hash(&mut h);
    h.finish()
}

impl Side {
    #[allow(dead_code)]
    fn as_f64(self) -> f64 {
        match self {
            Side::Buy => 1.0,
            Side::Sell => -1.0,
        }
    }
}

/// Map a fresh (bid, ask, last, vol) tuple onto the emissions appropriate
/// for the subscription mode. Streaming L1 emits four ticks per update;
/// TickByTick emits only the one kind requested; RealtimeBars5s is handled
/// elsewhere (bar aggregation, not tick-level).
fn emit_for_mode(
    key: &SubKey,
    mode: &SubMode,
    bid: f64,
    ask: f64,
    last: f64,
    last_size: i64,
    cum_volume: i64,
) -> Vec<MarketEmission> {
    match mode {
        SubMode::StreamingL1 { .. } => vec![
            MarketEmission::TickPrice {
                key: key.clone(),
                tick: TickType::Bid,
                price: bid,
                size: None,
                attribs: TickAttribs::default(),
            },
            MarketEmission::TickPrice {
                key: key.clone(),
                tick: TickType::Ask,
                price: ask,
                size: None,
                attribs: TickAttribs::default(),
            },
            MarketEmission::TickPrice {
                key: key.clone(),
                tick: TickType::Last,
                price: last,
                size: Some(last_size),
                attribs: TickAttribs::default(),
            },
            MarketEmission::TickSize {
                key: key.clone(),
                tick: TickType::Volume,
                size: cum_volume,
            },
        ],
        SubMode::TickByTick { kind } => match kind {
            TickByTickKind::Last | TickByTickKind::AllLast => vec![MarketEmission::TickPrice {
                key: key.clone(),
                tick: TickType::Last,
                price: last,
                size: Some(last_size),
                attribs: TickAttribs::default(),
            }],
            TickByTickKind::BidAsk => vec![
                MarketEmission::TickPrice {
                    key: key.clone(),
                    tick: TickType::Bid,
                    price: bid,
                    size: None,
                    attribs: TickAttribs::default(),
                },
                MarketEmission::TickPrice {
                    key: key.clone(),
                    tick: TickType::Ask,
                    price: ask,
                    size: None,
                    attribs: TickAttribs::default(),
                },
            ],
            TickByTickKind::MidPoint => vec![MarketEmission::TickPrice {
                key: key.clone(),
                tick: TickType::MarkPrice,
                price: (bid + ask) * 0.5,
                size: None,
                attribs: TickAttribs::default(),
            }],
        },
        // Real-time bars and historical batches are not emitted per-tick.
        SubMode::RealtimeBars5s | SubMode::Historical(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// MarketDataEngine impl
// ---------------------------------------------------------------------------

impl MarketDataEngine for SyntheticEngine {
    fn subscribe(&mut self, key: SubKey, mode: SubMode) -> Result<(), MarketDataError> {
        self.ensure_symbol(&key.symbol);
        self.pending_initial_emit.push(key.clone());
        self.subs.insert(key, mode);
        self.subs_order_dirty = true;
        Ok(())
    }

    fn unsubscribe(&mut self, key: &SubKey) {
        self.subs.remove(key);
        self.subs_order_dirty = true;
    }

    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
        self.refresh_subs_order();
        // Snapshot the symbol list up front, sorted for determinism.
        let mut symbols: Vec<SymbolKey> = self.symbols.keys().cloned().collect();
        symbols.sort_by_key(|s| (s.contract_id, s.symbol.clone()));
        let mut emissions = Vec::new();
        for sym in symbols {
            emissions.extend(self.advance_symbol(&sym, now));
        }
        emissions
    }

    fn snapshot(&self, symbol: &SymbolKey) -> Option<Snapshot> {
        self.last_snapshot.get(symbol).cloned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{ReqId, SessionId, TickByTickKind};

    fn sym(name: &str, cid: i32) -> SymbolKey {
        SymbolKey {
            contract_id: cid,
            symbol: name.into(),
        }
    }

    fn sub(session: u64, req: i32, s: SymbolKey) -> SubKey {
        SubKey {
            session: SessionId(session),
            req_id: ReqId(req),
            symbol: s,
        }
    }

    #[test]
    fn subscribe_and_step_produces_ticks() {
        let mut eng = SyntheticEngine::new(0xFEEDFACE);
        let s = sym("AAPL", 1);
        eng.register(s.clone(), SymbolPreset::Liquid, 175.0);
        eng.subscribe(
            sub(1, 1, s.clone()),
            SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        )
        .unwrap();
        // One minute of virtual time.
        let em = eng.step(VirtualInstant::from_secs(60));
        assert!(!em.is_empty(), "expected at least some tick emissions");
        let last = em
            .iter()
            .rev()
            .find_map(|e| match e {
                MarketEmission::TickPrice {
                    tick: TickType::Last,
                    price,
                    ..
                } => Some(*price),
                _ => None,
            })
            .expect("expect at least one Last emission");
        assert!((last - 175.0).abs() < 10.0, "price drifted wildly: {last}");
    }

    #[test]
    fn snapshot_returns_last_tick_state() {
        let mut eng = SyntheticEngine::new(7);
        let s = sym("SPY", 2);
        eng.register(s.clone(), SymbolPreset::Liquid, 500.0);
        eng.subscribe(
            sub(1, 1, s.clone()),
            SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        )
        .unwrap();
        let _ = eng.step(VirtualInstant::from_secs(10));
        let snap = eng.snapshot(&s).expect("snapshot after step");
        assert!(snap.bid < snap.ask);
        assert!(snap.last > 0.0);
    }

    #[test]
    fn determinism_same_seed_same_output() {
        let run = |seed: u64| {
            let mut eng = SyntheticEngine::new(seed);
            let s = sym("AAPL", 1);
            eng.register(s.clone(), SymbolPreset::Liquid, 100.0);
            eng.subscribe(
                sub(1, 1, s.clone()),
                SubMode::TickByTick {
                    kind: TickByTickKind::Last,
                },
            )
            .unwrap();
            eng.step(VirtualInstant::from_secs(30))
                .into_iter()
                .filter_map(|e| match e {
                    MarketEmission::TickPrice {
                        tick: TickType::Last,
                        price,
                        ..
                    } => Some(price.to_bits()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
    }

    #[test]
    fn halt_suppresses_emissions() {
        let mut eng = SyntheticEngine::new(1);
        let s = sym("AAPL", 1);
        eng.register(s.clone(), SymbolPreset::Liquid, 100.0);
        eng.subscribe(
            sub(1, 1, s.clone()),
            SubMode::TickByTick {
                kind: TickByTickKind::Last,
            },
        )
        .unwrap();
        // Warm up.
        let _ = eng.step(VirtualInstant::from_secs(1));
        eng.apply_halt(&s, VirtualInstant::from_secs(1), Duration::from_secs(5));
        let during_halt = eng.step(VirtualInstant::from_secs(3));
        assert!(during_halt.is_empty(), "halted symbol must not emit");
        let after = eng.step(VirtualInstant::from_secs(10));
        assert!(!after.is_empty(), "halt should lift after duration");
    }
}
