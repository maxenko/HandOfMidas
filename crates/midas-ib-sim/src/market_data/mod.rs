//! Market-data generator. Stage 03 owns the synthetic / replay / hybrid
//! engines; Stage 01 ships the `MarketDataEngine` trait + `MarketDataMode`
//! enum so other modules can name them.

pub mod generator;
pub mod hybrid;
pub mod replay;

use std::time::Duration;

use midas_broker_core::SymbolKey;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{MarketEmission, SubKey, SubMode};

/// Selected market-data source (CLI flag).
#[derive(Clone, Debug, Default)]
pub enum MarketDataMode {
    /// Pure synthetic (Roll-GARCH-U).
    #[default]
    Synthetic,
    /// Pure replay from a Databento `.dbn` file.
    Replay { path: std::path::PathBuf },
    /// Hybrid: replay with synthetic perturbations.
    Hybrid {
        replay: std::path::PathBuf,
        perturbation: String,
    },
}

/// Subscription-facing trait implemented by `SyntheticEngine`, `ReplayEngine`,
/// and `HybridEngine` (Stage 03). The engine actor calls `step()` each time
/// virtual time advances; each emission becomes an outbound wire message and
/// (for L1 ticks) a `MarketSnapshot` fed to the order simulator.
pub trait MarketDataEngine: Send {
    /// Open a new subscription. May fail if the contract is unknown.
    fn subscribe(&mut self, key: SubKey, mode: SubMode) -> Result<(), MarketDataError>;

    /// Close a subscription.
    fn unsubscribe(&mut self, key: &SubKey);

    /// Advance the generator to `now`, returning all emissions that fell due.
    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission>;

    /// Return the current best-effort snapshot for `symbol`, if any.
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Snapshot>;
}

/// Read-only snapshot returned from `MarketDataEngine::snapshot`.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub bid: f64,
    pub ask: f64,
    pub last: f64,
    pub volume: Option<i64>,
    pub ts: VirtualInstant,
}

/// Stage 03 error surface.
#[derive(Debug, thiserror::Error)]
pub enum MarketDataError {
    #[error("unknown contract")]
    UnknownContract,
    #[error("replay source exhausted")]
    ReplayExhausted,
    #[error("subscription cap exceeded")]
    CapExceeded,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Scripted perturbations injected into the hybrid engine. Post-processes the
/// base stream without mutating the base engine's own state.
#[derive(Clone, Debug)]
pub enum Perturbation {
    /// Add an instantaneous log-return jump to `symbol` at `at`.
    InjectJump {
        at: VirtualInstant,
        symbol: SymbolKey,
        magnitude_pct: f64,
    },
    /// Snap `symbol`'s price from `from` to `to` at `at`.
    InjectGap {
        at: VirtualInstant,
        symbol: SymbolKey,
        from: f64,
        to: f64,
    },
    /// Halt `symbol` for `duration` starting at `at` — suppresses emissions.
    InjectHalt {
        at: VirtualInstant,
        symbol: SymbolKey,
        duration: Duration,
    },
    /// Multiply arrival-rate by `multiplier` for every symbol over [from, to].
    BurstMode {
        from: VirtualInstant,
        to: VirtualInstant,
        multiplier: f64,
    },
}

impl Perturbation {
    pub fn when(&self) -> VirtualInstant {
        match self {
            Self::InjectJump { at, .. }
            | Self::InjectGap { at, .. }
            | Self::InjectHalt { at, .. } => *at,
            Self::BurstMode { from, .. } => *from,
        }
    }
}
