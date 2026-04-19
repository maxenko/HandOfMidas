//! 100 streaming-L1 line cap + 5 tick-by-tick cap. Stage 05 fills in.

use crate::engine::types::{QuirkViolation, ReqId, SessionId};

#[derive(Default)]
pub struct LineLimiter {
    _priv: (),
}

impl LineLimiter {
    pub fn new() -> Self {
        Self { _priv: () }
    }

    pub fn reserve_l1(
        &mut self,
        _session: SessionId,
        _req_id: ReqId,
    ) -> Result<(), QuirkViolation> {
        todo!("Stage 05 — LineLimiter::reserve_l1")
    }

    pub fn release_l1(&mut self, _session: SessionId, _req_id: ReqId) {
        todo!("Stage 05 — LineLimiter::release_l1")
    }
}
