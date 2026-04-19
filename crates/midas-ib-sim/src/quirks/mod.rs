//! IB quirk models — rate limits, line caps, pacing, farm status.
//!
//! Stage 05 fills in each guard behind the [`QuirkGuard`] trait. Stage 01
//! shipped the trait + the `error_codes` table; this module brings the
//! per-quirk modules online and wires them behind [`CompositeQuirkGuard`].
//!
//! # Tiers
//!
//! * **T1** (always on by default): `msg_rate`, `line_limit`,
//!   `historical_pacing`, `farm_status` initial bulletins, connection
//!   lifecycle events, daily restart.
//! * **T2** (opt-in via [`config::QuirksConfig`]): duplicate order status,
//!   `reqMarketDataType`, periodic farm cycling, contract-details latency.
//!
//! All error codes route through [`error_codes`] so the pre-release capture
//! pass is a single-file diff.

pub mod config;
pub mod contract_latency;
pub mod duplicate_status;
pub mod error_codes;
pub mod farm_status;
pub mod historical_pacing;
pub mod line_limit;
pub mod market_data_type;
pub mod msg_rate;

use std::sync::Arc;

use crate::engine::clock::Clock;
use crate::engine::types::{HistoricalReq, QuirkViolation, ReqId, SessionId};

pub use config::QuirksConfig;
pub use farm_status::{ConnEvent, FarmBulletin, FarmStatusEmitter};
pub use historical_pacing::{HistoricalPacing, PacingParams};
pub use line_limit::{LineLimiter, TickByTickLimiter};
pub use market_data_type::MarketDataTypePolicy;
pub use msg_rate::MsgRateLimiter;

use midas_broker_core::SymbolKey;

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
    L1Subscribe { symbol: &'a SymbolKey },
    L1Unsubscribe,
    TickByTickSubscribe { symbol: &'a SymbolKey },
    HistoricalRequest { req: &'a HistoricalReq },
    OrderPlace,
}

/// Stage-01 placeholder guard — used by the engine in tests that don't need
/// any quirk enforcement.
#[derive(Default)]
pub struct NoopQuirkGuard;

impl QuirkGuard for NoopQuirkGuard {
    fn check(&mut self, _ctx: QuirkCheckCtx<'_>) -> Result<(), QuirkViolation> {
        Ok(())
    }
    fn record(&mut self, _ctx: QuirkCheckCtx<'_>) {}
}

/// Production composite — wires msg-rate, L1 line cap, tick-by-tick cap, and
/// historical pacing into one `QuirkGuard` impl. The engine holds this as
/// `Box<dyn QuirkGuard>`.
///
/// Every ordinary client message flows through `check(MsgRate)` before any
/// subscription/order/historical path; the sub-specific check runs next and
/// fails closed on the first violation. The msg-rate bucket is *only* debited
/// on success to mirror real-IB semantics where violations disconnect before
/// further debits matter.
pub struct CompositeQuirkGuard {
    msg_rate: MsgRateLimiter,
    line_limit: LineLimiter,
    tick_by_tick: TickByTickLimiter,
    historical: HistoricalPacing,
}

impl CompositeQuirkGuard {
    /// Build a composite guard from a `QuirksConfig`. The `clock` is shared
    /// across every sub-limiter so virtual-time semantics are consistent.
    pub fn from_config(clock: Arc<dyn Clock>, cfg: &QuirksConfig) -> Self {
        let msg_rate = MsgRateLimiter::with_params(
            clock.clone(),
            cfg.msg_rate.limit_per_sec,
            cfg.msg_rate.limit_per_sec as f64,
        );
        let line_limit = LineLimiter::with_cap(cfg.line_limit.max_l1_lines as usize);
        let tick_by_tick = TickByTickLimiter::with_params(
            clock.clone(),
            cfg.line_limit.max_tbt as usize,
            cfg.line_limit.tbt_cooldown(),
        );
        let historical =
            HistoricalPacing::with_params(clock, cfg.historical_pacing.to_pacing_params());
        Self {
            msg_rate,
            line_limit,
            tick_by_tick,
            historical,
        }
    }

    /// Drop every per-session bookkeeping row for `session`. Called on
    /// disconnect.
    pub fn forget_session(&mut self, session: SessionId) {
        self.msg_rate.forget_session(session);
        self.line_limit.forget_session(session);
        self.tick_by_tick.forget_session(session);
        self.historical.forget_session(session);
    }

    /// Release an L1 line slot on explicit unsubscribe.
    pub fn release_l1(&mut self, session: SessionId, req_id: ReqId) {
        self.line_limit.release_l1(session, req_id);
    }

    /// Release a tick-by-tick slot (cooldown timestamp is intentionally
    /// preserved so re-subscribes within 15s still trip).
    pub fn release_tbt(&mut self, session: SessionId, req_id: ReqId) {
        self.tick_by_tick.release(session, req_id);
    }

    // Accessors for tests / snapshot projection.
    pub fn msg_rate(&self) -> &MsgRateLimiter {
        &self.msg_rate
    }
    pub fn line_limit(&self) -> &LineLimiter {
        &self.line_limit
    }
    pub fn tick_by_tick(&self) -> &TickByTickLimiter {
        &self.tick_by_tick
    }
    pub fn historical(&self) -> &HistoricalPacing {
        &self.historical
    }
}

impl QuirkGuard for CompositeQuirkGuard {
    fn check(&mut self, ctx: QuirkCheckCtx<'_>) -> Result<(), QuirkViolation> {
        // 1. Msg-rate is always checked first — every client frame costs a
        // token. On violation the session is torn down regardless of what
        // the sub-specific checks would have said.
        self.msg_rate.check(ctx.session)?;

        // 2. Sub-specific checks.
        match ctx.kind {
            QuirkCheckKind::MsgRate => Ok(()),
            QuirkCheckKind::L1Subscribe { .. } => {
                let req_id = ctx.req_id.unwrap_or_default();
                self.line_limit.reserve_l1(ctx.session, req_id)
            }
            QuirkCheckKind::L1Unsubscribe => {
                if let Some(req_id) = ctx.req_id {
                    self.line_limit.release_l1(ctx.session, req_id);
                }
                Ok(())
            }
            QuirkCheckKind::TickByTickSubscribe { symbol } => {
                let req_id = ctx.req_id.unwrap_or_default();
                self.tick_by_tick
                    .reserve(ctx.session, req_id, symbol.clone())
            }
            QuirkCheckKind::HistoricalRequest { req } => self.historical.check(ctx.session, req),
            QuirkCheckKind::OrderPlace => Ok(()),
        }
    }

    fn record(&mut self, _ctx: QuirkCheckCtx<'_>) {
        // Each sub-limiter records on success inside its own `check` call;
        // no additional work needed here. The trait exists so future quirks
        // (e.g. "count snapshot requests") can hook in.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;
    use midas_broker_core::{ContractSpec, SymbolKey as BrokerSymbolKey};

    fn mk_guard() -> (Arc<VirtualClock>, CompositeQuirkGuard) {
        let clock = Arc::new(VirtualClock::new());
        let guard = CompositeQuirkGuard::from_config(
            clock.clone() as Arc<dyn Clock>,
            &QuirksConfig::default(),
        );
        (clock, guard)
    }

    fn sym(id: i32) -> BrokerSymbolKey {
        BrokerSymbolKey {
            contract_id: id,
            symbol: format!("S{id}"),
        }
    }

    #[test]
    fn msg_rate_check_independent_of_other_limiters() {
        let (_clock, mut g) = mk_guard();
        // 50 msg-rate checks on a single session — all admit.
        for _ in 0..50 {
            g.check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .unwrap();
        }
        // 51st trips RateLimit — not a LineLimit.
        let err = g
            .check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .unwrap_err();
        assert!(matches!(err, QuirkViolation::RateLimit { .. }));
    }

    #[test]
    fn l1_subscribe_consumes_a_line_and_trips_at_cap() {
        let clock = Arc::new(VirtualClock::new());
        let mut cfg = QuirksConfig::default();
        cfg.line_limit.max_l1_lines = 2;
        let mut g = CompositeQuirkGuard::from_config(clock as Arc<dyn Clock>, &cfg);
        for i in 0..2 {
            g.check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: Some(ReqId(i)),
                kind: QuirkCheckKind::L1Subscribe { symbol: &sym(i) },
            })
            .unwrap();
        }
        let err = g
            .check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: Some(ReqId(99)),
                kind: QuirkCheckKind::L1Subscribe { symbol: &sym(99) },
            })
            .unwrap_err();
        assert!(matches!(err, QuirkViolation::LineLimit { .. }));
    }

    #[test]
    fn historical_request_routes_to_pacing_guard() {
        let (clock, mut g) = mk_guard();
        let req = HistoricalReq {
            contract: ContractSpec::Stock {
                symbol: "AAPL".into(),
                exchange: "SMART".into(),
                currency: "USD".into(),
            },
            end_date_time: "".into(),
            duration: "1 D".into(),
            bar_size: "1 min".into(),
            what_to_show: "TRADES".into(),
            use_rth: true,
            format_date: 1,
            keep_up_to_date: false,
        };
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(0)),
            kind: QuirkCheckKind::HistoricalRequest { req: &req },
        })
        .unwrap();
        // Immediate duplicate within cooldown trips 162.
        clock.advance(crate::engine::clock::VirtualInstant::from_millis(100));
        let err = g
            .check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: Some(ReqId(1)),
                kind: QuirkCheckKind::HistoricalRequest { req: &req },
            })
            .unwrap_err();
        assert!(matches!(err, QuirkViolation::HistoricalPacing { .. }));
    }

    #[test]
    fn forget_session_clears_every_limiter() {
        let (_clock, mut g) = mk_guard();
        // Drain msg-rate for session 1.
        for _ in 0..50 {
            g.check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .unwrap();
        }
        assert!(g
            .check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .is_err());
        g.forget_session(SessionId(1));
        // Fresh bucket after forgetting.
        for _ in 0..50 {
            assert!(g
                .check(QuirkCheckCtx {
                    session: SessionId(1),
                    req_id: None,
                    kind: QuirkCheckKind::MsgRate,
                })
                .is_ok());
        }
    }

    #[test]
    fn noop_guard_never_violates() {
        let mut g = NoopQuirkGuard;
        for _ in 0..1_000 {
            g.check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .unwrap();
        }
    }
}
