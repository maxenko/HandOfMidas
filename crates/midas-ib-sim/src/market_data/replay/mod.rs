//! Replay / hybrid market-data engine. Stage 03 + 07 fill in.

pub mod dbn_reader;
pub mod recorder;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{MarketEmission, SubKey, SubMode};
use crate::market_data::{MarketDataEngine, MarketDataError, Snapshot};

/// Reads a Databento `.dbn` file and feeds ticks through the engine interface.
#[derive(Default)]
pub struct ReplayEngine {
    _priv: (),
}

impl ReplayEngine {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl MarketDataEngine for ReplayEngine {
    fn subscribe(&mut self, _key: SubKey, _mode: SubMode) -> Result<(), MarketDataError> {
        todo!("Stage 03 — ReplayEngine::subscribe")
    }
    fn unsubscribe(&mut self, _key: &SubKey) {
        todo!("Stage 03 — ReplayEngine::unsubscribe")
    }
    fn step(&mut self, _now: VirtualInstant) -> Vec<MarketEmission> {
        Vec::new()
    }
    fn snapshot(&self, _symbol: &midas_broker_core::SymbolKey) -> Option<Snapshot> {
        None
    }
}
