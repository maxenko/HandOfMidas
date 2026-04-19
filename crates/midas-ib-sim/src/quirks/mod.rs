//! IB quirk models — rate limits, line caps, pacing, farm status. Stage 05
//! fills in each guard. Stage 01 ships the `QuirkGuard` trait + the
//! `error_codes` table so the protocol layer can reference it today.

pub mod error_codes;
pub mod farm_status;
pub mod historical_pacing;
pub mod line_limit;
pub mod msg_rate;

use crate::engine::types::{QuirkViolation, ReqId, SessionId};

/// All T1 quirks implement this shared trait.
pub trait QuirkGuard: Send {
    /// Consult all installed guards; return the first violation or `Ok(())`.
    fn check(&mut self, ctx: QuirkCheckCtx<'_>) -> Result<(), QuirkViolation>;

    /// Record a successful admission (after `check` succeeded and the
    /// request was dispatched). Used for sliding-window bookkeeping.
    fn record(&mut self, ctx: QuirkCheckCtx<'_>);
}

/// Context passed to `QuirkGuard::check` / `record`.
#[derive(Clone, Debug)]
pub struct QuirkCheckCtx<'a> {
    pub session: SessionId,
    pub req_id: Option<ReqId>,
    pub kind: QuirkCheckKind<'a>,
}

#[derive(Clone, Debug)]
pub enum QuirkCheckKind<'a> {
    MsgRate,
    L1Subscribe,
    L1Unsubscribe,
    TickByTickSubscribe,
    HistoricalRequest { fingerprint: &'a str },
    OrderPlace,
}

/// Stage-01 placeholder guard. Stage 05 replaces it with a composite over
/// the per-quirk modules.
#[derive(Default)]
pub struct NoopQuirkGuard;

impl QuirkGuard for NoopQuirkGuard {
    fn check(&mut self, _ctx: QuirkCheckCtx<'_>) -> Result<(), QuirkViolation> {
        Ok(())
    }
    fn record(&mut self, _ctx: QuirkCheckCtx<'_>) {}
}
