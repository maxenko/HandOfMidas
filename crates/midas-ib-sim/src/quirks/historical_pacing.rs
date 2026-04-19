//! Historical-data pacing — three concurrent regimes.
//!
//! Real IB enforces three overlapping rate limits on `reqHistoricalData` and
//! violates any of them with error 162:
//!
//! 1. **Window**: 60 requests per rolling 10-minute window (BID_ASK counts
//!    double — implemented here via the `cost` path).
//! 2. **Burst**: at most 6 identical requests (same contract/exchange/tickType/
//!    barSize) in any 2-second window.
//! 3. **Cooldown**: any two identical requests must be >= 15 seconds apart.
//!
//! The violation code (162) is `[unverified]` in the plan table. Value + text
//! come from docs and research — the pre-release capture pass must confirm.
//!
//! # Memory
//!
//! The sliding window is a `VecDeque` of `(timestamp, key, cost)` pruned on
//! every `check` call. Plan kill-criteria caps per-session history at 1 MB;
//! the 60-per-10min regime means at most ~60 entries per session at steady
//! state, well within budget.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::ContractSpec;

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::types::{HistoricalReq, QuirkViolation, SessionId, ViolationAction};
use crate::quirks::error_codes;

/// Plan default: 60 requests per rolling 10 minutes (cost-weighted).
pub const DEFAULT_WINDOW_LIMIT: u32 = 60;
/// Plan default: 10-minute rolling window.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(600);
/// Plan default: 6 identical-request cap per 2-second burst.
pub const DEFAULT_BURST_LIMIT: u32 = 6;
/// Plan default: 2-second burst window.
pub const DEFAULT_BURST_WINDOW: Duration = Duration::from_secs(2);
/// Plan default: 15-second identical-request cooldown.
pub const DEFAULT_IDENTICAL_COOLDOWN: Duration = Duration::from_secs(15);

/// Stable fingerprint for "is this request identical to another".
///
/// IB pacing keys on (contract, exchange, tick type, bar size); two requests
/// that differ only in `end_date_time` or `duration` still hit the same
/// underlying historical-data endpoint and so count as "identical". We hash
/// on a flattened projection of `ContractSpec` — `Stock`, `Option`, `Future`,
/// `Forex` — and ignore fields the pacing engine can't observe (contract id
/// isn't part of the wire key for this regime; exchange is).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct RequestKey {
    symbol: String,
    exchange: String,
    what_to_show: String,
    bar_size: String,
}

impl RequestKey {
    pub fn from(req: &HistoricalReq) -> Self {
        let (symbol, exchange) = contract_fields(&req.contract);
        Self {
            symbol,
            exchange,
            what_to_show: req.what_to_show.clone(),
            bar_size: req.bar_size.clone(),
        }
    }
}

fn contract_fields(c: &ContractSpec) -> (String, String) {
    match c {
        ContractSpec::Stock {
            symbol, exchange, ..
        } => (symbol.clone(), exchange.clone()),
        ContractSpec::Option {
            symbol, exchange, ..
        } => (symbol.clone(), exchange.clone()),
        ContractSpec::Future {
            symbol, exchange, ..
        } => (symbol.clone(), exchange.clone()),
        ContractSpec::Forex { pair } => (pair.clone(), "IDEALPRO".into()),
    }
}

#[derive(Clone, Debug)]
struct WindowEntry {
    ts: VirtualInstant,
    key: RequestKey,
    cost: u32,
}

#[derive(Clone, Default)]
struct SessionState {
    window: VecDeque<WindowEntry>,
    window_cost: u32,
    identical_cooldown: BTreeMap<RequestKey, VirtualInstant>,
    violations: u32,
}

/// Historical pacing guard.
#[derive(Clone)]
pub struct HistoricalPacing {
    clock: Arc<dyn Clock>,
    per_session: BTreeMap<SessionId, SessionState>,
    window_limit: u32,
    window: Duration,
    burst_limit: u32,
    burst_window: Duration,
    identical_cooldown: Duration,
    bidask_double_count: bool,
}

impl HistoricalPacing {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_params(clock, PacingParams::default())
    }

    pub fn with_params(clock: Arc<dyn Clock>, params: PacingParams) -> Self {
        assert!(params.window_limit > 0, "window_limit must be > 0");
        assert!(params.burst_limit > 0, "burst_limit must be > 0");
        Self {
            clock,
            per_session: BTreeMap::new(),
            window_limit: params.window_limit,
            window: params.window,
            burst_limit: params.burst_limit,
            burst_window: params.burst_window,
            identical_cooldown: params.identical_cooldown,
            bidask_double_count: params.bidask_double_count,
        }
    }

    /// Consult all three regimes. On success, the request is *recorded* — the
    /// caller does not need to call a separate `record` method.
    pub fn check(&mut self, session: SessionId, req: &HistoricalReq) -> Result<(), QuirkViolation> {
        let now = self.clock.now();
        let key = RequestKey::from(req);
        let cost = self.cost_for(req);

        let state = self.per_session.entry(session).or_default();
        prune_window(state, now, self.window);

        // Regime 1 — rolling 10-minute window (cost-weighted).
        if state.window_cost + cost > self.window_limit {
            state.violations += 1;
            return Err(violation(
                "Historical data pacing violation (rolling window)",
            ));
        }

        // Regime 2 — 6 identical in 2s.
        let identical_burst = state
            .window
            .iter()
            .filter(|e| e.key == key && now.saturating_sub(e.ts) < self.burst_window)
            .count();
        if identical_burst as u32 >= self.burst_limit {
            state.violations += 1;
            return Err(violation("Historical data pacing violation (burst)"));
        }

        // Regime 3 — 15s identical cooldown.
        if let Some(&last) = state.identical_cooldown.get(&key) {
            if now.saturating_sub(last) < self.identical_cooldown {
                state.violations += 1;
                return Err(violation("Historical data pacing violation (cooldown)"));
            }
        }

        // Admit and record.
        state.window.push_back(WindowEntry {
            ts: now,
            key: key.clone(),
            cost,
        });
        state.window_cost += cost;
        state.identical_cooldown.insert(key, now);
        Ok(())
    }

    /// Drop session bookkeeping — called from `forget_session` on disconnect.
    pub fn forget_session(&mut self, session: SessionId) {
        self.per_session.remove(&session);
    }

    /// Read-only: violations observed for `session`. Used by tests.
    pub fn violations(&self, session: SessionId) -> u32 {
        self.per_session
            .get(&session)
            .map(|s| s.violations)
            .unwrap_or(0)
    }

    /// Total violations across every session — for `EngineSnapshot.quirks`.
    pub fn total_violations(&self) -> u64 {
        self.per_session.values().map(|s| s.violations as u64).sum()
    }

    /// Cost this request contributes to the 10-minute budget. BID_ASK counts
    /// double when the feature is on (default).
    pub fn cost_for(&self, req: &HistoricalReq) -> u32 {
        if self.bidask_double_count && req.what_to_show.eq_ignore_ascii_case("BID_ASK") {
            2
        } else {
            1
        }
    }
}

/// Tuning knobs surfaced to [`QuirksConfig`] — see `config.rs`.
#[derive(Copy, Clone, Debug)]
pub struct PacingParams {
    pub window_limit: u32,
    pub window: Duration,
    pub burst_limit: u32,
    pub burst_window: Duration,
    pub identical_cooldown: Duration,
    pub bidask_double_count: bool,
}

impl Default for PacingParams {
    fn default() -> Self {
        Self {
            window_limit: DEFAULT_WINDOW_LIMIT,
            window: DEFAULT_WINDOW,
            burst_limit: DEFAULT_BURST_LIMIT,
            burst_window: DEFAULT_BURST_WINDOW,
            identical_cooldown: DEFAULT_IDENTICAL_COOLDOWN,
            bidask_double_count: true,
        }
    }
}

fn prune_window(state: &mut SessionState, now: VirtualInstant, window: Duration) {
    while let Some(front) = state.window.front() {
        if now.saturating_sub(front.ts) > window {
            // Evicting — decrement window_cost by the evicted entry's cost.
            let evicted = state.window.pop_front().expect("peeked, must pop");
            state.window_cost = state.window_cost.saturating_sub(evicted.cost);
        } else {
            break;
        }
    }
}

fn violation(msg: &str) -> QuirkViolation {
    QuirkViolation::HistoricalPacing {
        code: error_codes::HISTORICAL_PACING,
        message: format!(
            "{}: {}",
            error_codes::message(error_codes::HISTORICAL_PACING),
            msg
        ),
        action: ViolationAction::RejectRequest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;
    use midas_broker_core::ContractSpec;

    fn mk_req(symbol: &str, what_to_show: &str, bar_size: &str) -> HistoricalReq {
        HistoricalReq {
            contract: ContractSpec::Stock {
                symbol: symbol.into(),
                exchange: "SMART".into(),
                currency: "USD".into(),
            },
            end_date_time: "".into(),
            duration: "1 D".into(),
            bar_size: bar_size.into(),
            what_to_show: what_to_show.into(),
            use_rth: true,
            format_date: 1,
            keep_up_to_date: false,
        }
    }

    fn mk() -> (Arc<VirtualClock>, HistoricalPacing) {
        let clock = Arc::new(VirtualClock::new());
        let pacing = HistoricalPacing::new(clock.clone() as Arc<dyn Clock>);
        (clock, pacing)
    }

    // -----------------------------------------------------------------------
    // Regime 1 — rolling window.
    // -----------------------------------------------------------------------

    #[test]
    fn window_admits_exactly_sixty_distinct() {
        let (clock, mut p) = mk();
        // 60 distinct requests, spaced 1s apart to avoid burst/cooldown trips.
        for i in 0..60 {
            clock.advance(VirtualInstant::from_millis(i * 1_000));
            let req = mk_req(&format!("SYM{i}"), "TRADES", "1 min");
            assert!(p.check(SessionId(1), &req).is_ok(), "i={i}");
        }
    }

    #[test]
    fn sixty_first_request_trips_162() {
        let (clock, mut p) = mk();
        for i in 0..60 {
            clock.advance(VirtualInstant::from_millis(i * 1_000));
            p.check(SessionId(1), &mk_req(&format!("SYM{i}"), "TRADES", "1 min"))
                .unwrap();
        }
        clock.advance(VirtualInstant::from_millis(61_000));
        let err = p
            .check(SessionId(1), &mk_req("SYMX", "TRADES", "1 min"))
            .unwrap_err();
        match err {
            QuirkViolation::HistoricalPacing { code, action, .. } => {
                assert_eq!(code, error_codes::HISTORICAL_PACING);
                assert_eq!(action, ViolationAction::RejectRequest);
            }
            other => panic!("expected HistoricalPacing, got {other:?}"),
        }
    }

    #[test]
    fn window_evicts_after_ten_minutes() {
        let (clock, mut p) = mk();
        for i in 0..60 {
            clock.advance(VirtualInstant::from_millis(i * 1_000));
            p.check(SessionId(1), &mk_req(&format!("SYM{i}"), "TRADES", "1 min"))
                .unwrap();
        }
        // Jump past the 10-minute window — all entries age out.
        clock.advance(VirtualInstant::from_secs(1_000));
        assert!(p
            .check(SessionId(1), &mk_req("FRESH", "TRADES", "1 min"))
            .is_ok());
    }

    #[test]
    fn bidask_counts_double_against_window() {
        let (clock, mut p) = mk();
        // 30 BID_ASK requests = 60 cost units -> the cap.
        for i in 0..30 {
            clock.advance(VirtualInstant::from_millis(i * 1_000));
            p.check(SessionId(1), &mk_req(&format!("B{i}"), "BID_ASK", "1 min"))
                .unwrap();
        }
        // 31st BID_ASK (+2) blows the 60 cap.
        clock.advance(VirtualInstant::from_secs(35));
        assert!(p
            .check(SessionId(1), &mk_req("BX", "BID_ASK", "1 min"))
            .is_err());
    }

    #[test]
    fn bidask_flag_can_be_disabled() {
        let clock = Arc::new(VirtualClock::new());
        let mut p = HistoricalPacing::with_params(
            clock.clone() as Arc<dyn Clock>,
            PacingParams {
                bidask_double_count: false,
                ..PacingParams::default()
            },
        );
        // 60 BID_ASK requests at 1/sec — under a non-doubling regime all fit.
        for i in 0..60 {
            clock.advance(VirtualInstant::from_millis(i * 1_000));
            assert!(p
                .check(SessionId(1), &mk_req(&format!("B{i}"), "BID_ASK", "1 min"))
                .is_ok());
        }
    }

    // -----------------------------------------------------------------------
    // Regime 2 — 6-in-2s burst on identical requests.
    // -----------------------------------------------------------------------

    #[test]
    fn burst_allows_five_identical_within_limit() {
        // Disable the cooldown so we isolate the burst regime. With
        // burst_limit=6 and burst_window=2s, five identical requests spaced
        // 100ms apart must all admit.
        let clock = Arc::new(VirtualClock::new());
        let mut p = HistoricalPacing::with_params(
            clock.clone() as Arc<dyn Clock>,
            PacingParams {
                identical_cooldown: Duration::from_millis(0),
                ..PacingParams::default()
            },
        );
        let req = mk_req("AAPL", "TRADES", "1 min");
        for i in 0..5 {
            clock.advance(VirtualInstant::from_millis(i * 100 + 1));
            assert!(p.check(SessionId(1), &req).is_ok(), "i={i}");
        }
    }

    #[test]
    fn six_identical_in_two_seconds_trips_burst() {
        // Defeat the 15s cooldown (set to 0) so burst alone drives the trip.
        let clock = Arc::new(VirtualClock::new());
        let mut p = HistoricalPacing::with_params(
            clock.clone() as Arc<dyn Clock>,
            PacingParams {
                identical_cooldown: Duration::from_millis(0),
                ..PacingParams::default()
            },
        );
        let req = mk_req("AAPL", "TRADES", "1 min");
        for _ in 0..6 {
            // Must advance at least 1ms between to keep the cooldown=0 from
            // short-circuiting, but stay inside the 2s burst window.
            clock.advance(VirtualInstant::from_millis(1));
            p.check(SessionId(1), &req).unwrap();
        }
        clock.advance(VirtualInstant::from_millis(1));
        let err = p.check(SessionId(1), &req).unwrap_err();
        assert!(matches!(err, QuirkViolation::HistoricalPacing { .. }));
    }

    // -----------------------------------------------------------------------
    // Regime 3 — 15s identical cooldown.
    // -----------------------------------------------------------------------

    #[test]
    fn identical_within_15s_is_blocked() {
        let (clock, mut p) = mk();
        let req = mk_req("AAPL", "TRADES", "1 min");
        p.check(SessionId(1), &req).unwrap();
        clock.advance(VirtualInstant::from_secs(5));
        let err = p.check(SessionId(1), &req).unwrap_err();
        assert!(matches!(err, QuirkViolation::HistoricalPacing { .. }));
    }

    #[test]
    fn identical_after_15s_is_allowed() {
        let (clock, mut p) = mk();
        let req = mk_req("AAPL", "TRADES", "1 min");
        p.check(SessionId(1), &req).unwrap();
        clock.advance(VirtualInstant::from_secs(15));
        // Now allowed.
        assert!(p.check(SessionId(1), &req).is_ok());
    }

    #[test]
    fn non_identical_ignores_cooldown() {
        let (clock, mut p) = mk();
        p.check(SessionId(1), &mk_req("AAPL", "TRADES", "1 min"))
            .unwrap();
        clock.advance(VirtualInstant::from_millis(100));
        // Different symbol — no cooldown.
        assert!(p
            .check(SessionId(1), &mk_req("MSFT", "TRADES", "1 min"))
            .is_ok());
    }

    #[test]
    fn sessions_are_independent() {
        let (clock, mut p) = mk();
        let req = mk_req("AAPL", "TRADES", "1 min");
        p.check(SessionId(1), &req).unwrap();
        // Session 2 is untouched.
        clock.advance(VirtualInstant::from_millis(50));
        assert!(p.check(SessionId(2), &req).is_ok());
    }

    #[test]
    fn three_regimes_report_distinct_messages() {
        // Sanity: each regime returns a different human-readable suffix so
        // operators can tell them apart in logs.
        let clock = Arc::new(VirtualClock::new());
        let mut p = HistoricalPacing::with_params(
            clock.clone() as Arc<dyn Clock>,
            PacingParams {
                identical_cooldown: Duration::from_millis(0),
                ..PacingParams::default()
            },
        );
        let req = mk_req("AAPL", "TRADES", "1 min");
        for _ in 0..6 {
            clock.advance(VirtualInstant::from_millis(1));
            p.check(SessionId(1), &req).unwrap();
        }
        clock.advance(VirtualInstant::from_millis(1));
        let err = p.check(SessionId(1), &req).unwrap_err();
        match err {
            QuirkViolation::HistoricalPacing { message, .. } => {
                assert!(message.contains("burst"), "expected burst tag in {message}");
            }
            other => panic!("got {other:?}"),
        }
    }
}
