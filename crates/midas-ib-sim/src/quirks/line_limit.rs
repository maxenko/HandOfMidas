//! Streaming-line caps.
//!
//! Two limiters live here because real IB enforces them independently:
//!
//! * [`LineLimiter`] — 100 concurrent L1 lines per session (default). Snapshot
//!   subscriptions are exempt. Overflow emits `10197` and the request is
//!   rejected; the session stays open.
//! * [`TickByTickLimiter`] — 5 concurrent tick-by-tick subscriptions per
//!   session, with a 15-second cooldown per `(session, contract_id)` pair.
//!
//! Both limiters track subscriptions via `BTreeSet` / `BTreeMap` for ordered
//! iteration in tests and deterministic `EngineSnapshot` projection.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::SymbolKey;

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::types::{QuirkViolation, ReqId, SessionId, ViolationAction};
use crate::quirks::error_codes;

/// Default cap on concurrent L1 streaming lines per session.
pub const DEFAULT_MAX_L1_LINES: usize = 100;

/// Default cap on concurrent tick-by-tick subscriptions per session.
pub const DEFAULT_MAX_TBT_LINES: usize = 5;

/// Default cooldown between consecutive tick-by-tick subscriptions for the
/// same `(session, contract_id)` — real IB rejects with 10197 if violated.
pub const DEFAULT_TBT_COOLDOWN: Duration = Duration::from_secs(15);

/// 100-L1-ticker line cap per session.
#[derive(Clone)]
pub struct LineLimiter {
    streaming_lines: BTreeMap<SessionId, BTreeSet<ReqId>>,
    max_lines_per_session: usize,
}

impl Default for LineLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl LineLimiter {
    pub fn new() -> Self {
        Self::with_cap(DEFAULT_MAX_L1_LINES)
    }

    pub fn with_cap(max_lines_per_session: usize) -> Self {
        assert!(max_lines_per_session > 0, "LineLimiter cap must be > 0");
        Self {
            streaming_lines: BTreeMap::new(),
            max_lines_per_session,
        }
    }

    /// Attempt to reserve an L1 line for `(session, req_id)`.
    ///
    /// If the request is already tracked, this is a no-op (re-send from the
    /// same `req_id` shouldn't double-count). Overflow emits error 10197 and
    /// the protocol layer rejects the request without touching the session.
    pub fn reserve_l1(&mut self, session: SessionId, req_id: ReqId) -> Result<(), QuirkViolation> {
        let lines = self.streaming_lines.entry(session).or_default();
        if lines.contains(&req_id) {
            return Ok(());
        }
        if lines.len() >= self.max_lines_per_session {
            return Err(QuirkViolation::LineLimit {
                code: error_codes::LINE_CAP_OVERFLOW,
                message: error_codes::message(error_codes::LINE_CAP_OVERFLOW).to_string(),
                action: ViolationAction::RejectRequest,
            });
        }
        lines.insert(req_id);
        Ok(())
    }

    /// Release an L1 line — called on explicit unsubscribe or session close.
    pub fn release_l1(&mut self, session: SessionId, req_id: ReqId) {
        if let Some(lines) = self.streaming_lines.get_mut(&session) {
            lines.remove(&req_id);
            if lines.is_empty() {
                self.streaming_lines.remove(&session);
            }
        }
    }

    /// Drop every line owned by `session`.
    pub fn forget_session(&mut self, session: SessionId) {
        self.streaming_lines.remove(&session);
    }

    /// Current count of active lines for `session`.
    pub fn active_lines(&self, session: SessionId) -> usize {
        self.streaming_lines
            .get(&session)
            .map(|s| s.len())
            .unwrap_or(0)
    }
}

/// 5-symbol tick-by-tick cap + 15-second per-instrument cooldown per session.
#[derive(Clone)]
pub struct TickByTickLimiter {
    clock: Arc<dyn Clock>,
    active: BTreeMap<SessionId, BTreeMap<ReqId, SymbolKey>>,
    /// Last time a tick-by-tick subscription was accepted for
    /// `(session, contract_id)`. Used to enforce the 15s cooldown.
    last_sub: BTreeMap<(SessionId, i32), VirtualInstant>,
    max_tbt: usize,
    cooldown: Duration,
}

impl TickByTickLimiter {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_params(clock, DEFAULT_MAX_TBT_LINES, DEFAULT_TBT_COOLDOWN)
    }

    pub fn with_params(clock: Arc<dyn Clock>, max_tbt: usize, cooldown: Duration) -> Self {
        assert!(max_tbt > 0, "TickByTickLimiter cap must be > 0");
        Self {
            clock,
            active: BTreeMap::new(),
            last_sub: BTreeMap::new(),
            max_tbt,
            cooldown,
        }
    }

    /// Try to reserve a tick-by-tick slot.
    ///
    /// Two ways to fail:
    /// 1. The session already has [`max_tbt`](Self::max_tbt) active
    ///    subscriptions (emits 10197 — plan treats the TBT cap as another
    ///    line-cap trigger).
    /// 2. A previous subscription for the same `contract_id` was accepted
    ///    within the past [`cooldown`](Self::cooldown).
    ///
    /// Idempotent: re-subscribing with the same `req_id` is a no-op.
    pub fn reserve(
        &mut self,
        session: SessionId,
        req_id: ReqId,
        symbol: SymbolKey,
    ) -> Result<(), QuirkViolation> {
        let now = self.clock.now();

        // Idempotency: same req_id re-sent.
        if let Some(per) = self.active.get(&session) {
            if per.contains_key(&req_id) {
                return Ok(());
            }
        }

        // Per-instrument cooldown.
        let key = (session, symbol.contract_id);
        if let Some(&last) = self.last_sub.get(&key) {
            let since = now.saturating_sub(last);
            if since < self.cooldown {
                return Err(QuirkViolation::TickByTickLimit {
                    code: error_codes::LINE_CAP_OVERFLOW,
                    message: format!(
                        "Tick-by-tick cooldown: re-subscribe to {} not allowed for another {:?}",
                        symbol.symbol,
                        self.cooldown - since,
                    ),
                    action: ViolationAction::RejectRequest,
                });
            }
        }

        let per = self.active.entry(session).or_default();
        if per.len() >= self.max_tbt {
            return Err(QuirkViolation::TickByTickLimit {
                code: error_codes::LINE_CAP_OVERFLOW,
                message: format!("Max tick-by-tick subscriptions ({}) reached", self.max_tbt),
                action: ViolationAction::RejectRequest,
            });
        }

        per.insert(req_id, symbol);
        self.last_sub.insert(key, now);
        Ok(())
    }

    /// Release a tick-by-tick slot. Does *not* clear the cooldown timestamp —
    /// 15s must still elapse before a re-subscribe.
    pub fn release(&mut self, session: SessionId, req_id: ReqId) {
        if let Some(per) = self.active.get_mut(&session) {
            per.remove(&req_id);
            if per.is_empty() {
                self.active.remove(&session);
            }
        }
    }

    pub fn forget_session(&mut self, session: SessionId) {
        self.active.remove(&session);
        self.last_sub.retain(|(s, _), _| *s != session);
    }

    pub fn active_count(&self, session: SessionId) -> usize {
        self.active.get(&session).map(|s| s.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;

    // -----------------------------------------------------------------------
    // LineLimiter — 100 L1 cap.
    // -----------------------------------------------------------------------

    #[test]
    fn l1_admits_up_to_cap() {
        let mut lim = LineLimiter::new();
        for i in 0..100 {
            assert!(lim.reserve_l1(SessionId(1), ReqId(i)).is_ok());
        }
    }

    #[test]
    fn hundred_and_first_trips_10197() {
        let mut lim = LineLimiter::new();
        for i in 0..100 {
            lim.reserve_l1(SessionId(1), ReqId(i)).unwrap();
        }
        let err = lim.reserve_l1(SessionId(1), ReqId(100)).unwrap_err();
        match err {
            QuirkViolation::LineLimit { code, action, .. } => {
                assert_eq!(code, error_codes::LINE_CAP_OVERFLOW);
                assert_eq!(action, ViolationAction::RejectRequest);
            }
            other => panic!("expected LineLimit, got {other:?}"),
        }
        assert_eq!(lim.active_lines(SessionId(1)), 100);
    }

    #[test]
    fn release_makes_room() {
        let mut lim = LineLimiter::new();
        for i in 0..100 {
            lim.reserve_l1(SessionId(1), ReqId(i)).unwrap();
        }
        assert!(lim.reserve_l1(SessionId(1), ReqId(100)).is_err());
        lim.release_l1(SessionId(1), ReqId(0));
        // Now there's room for the 101st.
        assert!(lim.reserve_l1(SessionId(1), ReqId(100)).is_ok());
    }

    #[test]
    fn reserve_is_idempotent_for_same_req_id() {
        let mut lim = LineLimiter::with_cap(2);
        lim.reserve_l1(SessionId(1), ReqId(10)).unwrap();
        lim.reserve_l1(SessionId(1), ReqId(10)).unwrap(); // no-op
        assert_eq!(lim.active_lines(SessionId(1)), 1);
        lim.reserve_l1(SessionId(1), ReqId(11)).unwrap();
        // Cap is 2 — third distinct id trips.
        assert!(lim.reserve_l1(SessionId(1), ReqId(12)).is_err());
    }

    #[test]
    fn sessions_are_independent() {
        let mut lim = LineLimiter::with_cap(2);
        lim.reserve_l1(SessionId(1), ReqId(0)).unwrap();
        lim.reserve_l1(SessionId(1), ReqId(1)).unwrap();
        assert!(lim.reserve_l1(SessionId(1), ReqId(2)).is_err());
        // Different session has its own bucket.
        lim.reserve_l1(SessionId(2), ReqId(0)).unwrap();
    }

    #[test]
    fn forget_session_clears_lines() {
        let mut lim = LineLimiter::with_cap(2);
        lim.reserve_l1(SessionId(1), ReqId(0)).unwrap();
        lim.reserve_l1(SessionId(1), ReqId(1)).unwrap();
        lim.forget_session(SessionId(1));
        assert_eq!(lim.active_lines(SessionId(1)), 0);
    }

    // -----------------------------------------------------------------------
    // TickByTickLimiter — 5 cap + 15s cooldown.
    // -----------------------------------------------------------------------

    fn sym(contract_id: i32) -> SymbolKey {
        SymbolKey {
            contract_id,
            symbol: format!("S{contract_id}"),
        }
    }

    #[test]
    fn tbt_admits_up_to_five_distinct() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = TickByTickLimiter::new(clock as Arc<dyn Clock>);
        for i in 0..5 {
            assert!(lim.reserve(SessionId(1), ReqId(i), sym(i)).is_ok());
        }
    }

    #[test]
    fn sixth_tbt_subscription_trips() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = TickByTickLimiter::new(clock as Arc<dyn Clock>);
        for i in 0..5 {
            lim.reserve(SessionId(1), ReqId(i), sym(i)).unwrap();
        }
        let err = lim.reserve(SessionId(1), ReqId(5), sym(5)).unwrap_err();
        match err {
            QuirkViolation::TickByTickLimit { code, action, .. } => {
                assert_eq!(code, error_codes::LINE_CAP_OVERFLOW);
                assert_eq!(action, ViolationAction::RejectRequest);
            }
            other => panic!("expected TickByTickLimit, got {other:?}"),
        }
    }

    #[test]
    fn tbt_cooldown_blocks_resubscribe_within_15s() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = TickByTickLimiter::new(clock.clone() as Arc<dyn Clock>);
        lim.reserve(SessionId(1), ReqId(0), sym(42)).unwrap();
        lim.release(SessionId(1), ReqId(0));
        // Re-subscribe immediately — within cooldown, must reject.
        let err = lim.reserve(SessionId(1), ReqId(1), sym(42)).unwrap_err();
        assert!(matches!(err, QuirkViolation::TickByTickLimit { .. }));
        // VirtualClock::advance takes an absolute target. 14.9s — still cooling.
        clock.advance(VirtualInstant::from_millis(14_900));
        assert!(lim.reserve(SessionId(1), ReqId(1), sym(42)).is_err());
        // 15.1s — past cooldown, OK.
        clock.advance(VirtualInstant::from_millis(15_100));
        assert!(lim.reserve(SessionId(1), ReqId(1), sym(42)).is_ok());
    }

    #[test]
    fn tbt_cooldown_is_per_contract() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = TickByTickLimiter::new(clock as Arc<dyn Clock>);
        lim.reserve(SessionId(1), ReqId(0), sym(42)).unwrap();
        lim.release(SessionId(1), ReqId(0));
        // Different contract — cooldown for 42 doesn't block 99.
        assert!(lim.reserve(SessionId(1), ReqId(1), sym(99)).is_ok());
    }

    #[test]
    fn tbt_idempotent_req_id() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = TickByTickLimiter::new(clock as Arc<dyn Clock>);
        lim.reserve(SessionId(1), ReqId(0), sym(42)).unwrap();
        // Same req_id — no-op, not a cooldown violation.
        assert!(lim.reserve(SessionId(1), ReqId(0), sym(42)).is_ok());
        assert_eq!(lim.active_count(SessionId(1)), 1);
    }
}
