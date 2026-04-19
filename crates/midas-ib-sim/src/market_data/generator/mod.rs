//! Synthetic tick generator (Roll-GARCH-U). Stage 03 fills in.

pub mod garch;
pub mod hawkes;
pub mod roll;
pub mod u_shape;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{MarketEmission, SubKey, SubMode};
use crate::market_data::{MarketDataEngine, MarketDataError, Snapshot};

/// Synthetic engine — implements `MarketDataEngine` using the Roll-GARCH-U
/// model. Stage 03 fills the body.
#[derive(Default)]
pub struct SyntheticEngine {
    _priv: (),
}

impl SyntheticEngine {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl MarketDataEngine for SyntheticEngine {
    fn subscribe(&mut self, _key: SubKey, _mode: SubMode) -> Result<(), MarketDataError> {
        todo!("Stage 03 — SyntheticEngine::subscribe")
    }
    fn unsubscribe(&mut self, _key: &SubKey) {
        todo!("Stage 03 — SyntheticEngine::unsubscribe")
    }
    fn step(&mut self, _now: VirtualInstant) -> Vec<MarketEmission> {
        // Stage 01 returns empty; Stage 03 fills in the real step.
        Vec::new()
    }
    fn snapshot(&self, _symbol: &midas_broker_core::SymbolKey) -> Option<Snapshot> {
        None
    }
}
