//! Historical-data pacing: 60 per 10min + 6 identical in 2s + 15s cooldown.
//! Stage 05 fills in.

use crate::engine::types::{HistoricalReq, QuirkViolation, SessionId};

#[derive(Default)]
pub struct HistoricalPacing {
    _priv: (),
}

impl HistoricalPacing {
    pub fn new() -> Self {
        Self { _priv: () }
    }

    pub fn check(
        &mut self,
        _session: SessionId,
        _req: &HistoricalReq,
    ) -> Result<(), QuirkViolation> {
        todo!("Stage 05 — HistoricalPacing::check")
    }
}
