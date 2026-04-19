//! 50 msg/sec per-session rate limiter. Stage 05 fills in.

use crate::engine::types::{QuirkViolation, SessionId};

#[derive(Default)]
pub struct MsgRateLimiter {
    _priv: (),
}

impl MsgRateLimiter {
    pub fn new() -> Self {
        Self { _priv: () }
    }

    pub fn check(&mut self, _session: SessionId) -> Result<(), QuirkViolation> {
        todo!("Stage 05 — MsgRateLimiter::check")
    }
}
