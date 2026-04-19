//! Databento `.dbn` reader. Stage 03 fills in.

use std::path::Path;

use crate::market_data::MarketDataError;

/// Streaming reader over a `.dbn` file. Stage 03 implements.
pub struct DbnReader {
    _priv: (),
}

impl DbnReader {
    pub fn open(_path: &Path) -> Result<Self, MarketDataError> {
        todo!("Stage 03 — DbnReader::open")
    }
}
