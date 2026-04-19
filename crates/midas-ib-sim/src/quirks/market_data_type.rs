//! T2 quirk — `reqMarketDataType` live / frozen / delayed / delayed-frozen.
//!
//! Real IB lets a client flip the streaming data mode mid-session. Each mode
//! has distinct tick-emission semantics:
//!
//! * **Live**: normal tick stream at current virtual time.
//! * **Frozen**: last-seen snapshot; no further updates.
//! * **Delayed**: subtract 15 min from virtual time; tick values are taken
//!   from the live feed's history.
//! * **Delayed-frozen**: Delayed + no updates.
//!
//! This module owns *policy* — the mapping from config mode to tick-emission
//! decisions. The market-data engine owns execution.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{MarketDataType, SessionId};
use crate::quirks::config::{MarketDataTypeConfig, MarketDataTypeKind};

/// Per-session market-data-type state with per-session override support.
///
/// Interior mutability via `Mutex` so the engine can hold the policy behind
/// `Arc` and share it with protocol-layer callbacks without plumbing `&mut`.
#[derive(Debug)]
pub struct MarketDataTypePolicy {
    default_kind: MarketDataTypeKind,
    delayed_lag: Duration,
    overrides: Mutex<std::collections::BTreeMap<SessionId, MarketDataTypeKind>>,
}

impl MarketDataTypePolicy {
    pub fn new(cfg: &MarketDataTypeConfig) -> Self {
        Self {
            default_kind: cfg.default,
            delayed_lag: Duration::from_secs(cfg.delayed_lag_secs),
            overrides: Mutex::new(Default::default()),
        }
    }

    pub fn shared(cfg: &MarketDataTypeConfig) -> Arc<Self> {
        Arc::new(Self::new(cfg))
    }

    /// Record a `reqMarketDataType` for `session`.
    pub fn set_for_session(&self, session: SessionId, ty: MarketDataType) {
        let kind = match ty {
            MarketDataType::Live => MarketDataTypeKind::Live,
            MarketDataType::Frozen => MarketDataTypeKind::Frozen,
            MarketDataType::Delayed => MarketDataTypeKind::Delayed,
            MarketDataType::DelayedFrozen => MarketDataTypeKind::DelayedFrozen,
        };
        self.overrides.lock().unwrap().insert(session, kind);
    }

    /// Current mode for `session` — falls back to the configured default.
    pub fn kind_for(&self, session: SessionId) -> MarketDataTypeKind {
        self.overrides
            .lock()
            .unwrap()
            .get(&session)
            .copied()
            .unwrap_or(self.default_kind)
    }

    /// Convert a live-feed tick timestamp to the emission timestamp for
    /// `session`, applying the 15-min shift for delayed modes.
    pub fn timestamp_for(&self, session: SessionId, live_ts: VirtualInstant) -> VirtualInstant {
        match self.kind_for(session) {
            MarketDataTypeKind::Live | MarketDataTypeKind::Frozen => live_ts,
            MarketDataTypeKind::Delayed | MarketDataTypeKind::DelayedFrozen => {
                VirtualInstant::from_duration(
                    live_ts.as_duration().saturating_sub(self.delayed_lag),
                )
            }
        }
    }

    /// Should we emit further tick updates for `session`? Frozen modes freeze
    /// at the last seen value.
    pub fn allows_updates(&self, session: SessionId) -> bool {
        !matches!(
            self.kind_for(session),
            MarketDataTypeKind::Frozen | MarketDataTypeKind::DelayedFrozen
        )
    }

    pub fn forget_session(&self, session: SessionId) {
        self.overrides.lock().unwrap().remove(&session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_live() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        assert_eq!(p.kind_for(SessionId(1)), MarketDataTypeKind::Live);
        assert!(p.allows_updates(SessionId(1)));
        let ts = VirtualInstant::from_secs(3_600);
        assert_eq!(p.timestamp_for(SessionId(1), ts), ts);
    }

    #[test]
    fn set_to_frozen_freezes_updates() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::Frozen);
        assert!(!p.allows_updates(SessionId(1)));
        let ts = VirtualInstant::from_secs(60);
        assert_eq!(p.timestamp_for(SessionId(1), ts), ts);
    }

    #[test]
    fn set_to_delayed_shifts_timestamp() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::Delayed);
        let ts = VirtualInstant::from_secs(3_600);
        let shifted = p.timestamp_for(SessionId(1), ts);
        assert_eq!(shifted.as_duration(), Duration::from_secs(3_600 - 900));
        assert!(p.allows_updates(SessionId(1)));
    }

    #[test]
    fn delayed_frozen_shifts_and_freezes() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::DelayedFrozen);
        let ts = VirtualInstant::from_secs(3_600);
        let shifted = p.timestamp_for(SessionId(1), ts);
        assert_eq!(shifted.as_duration(), Duration::from_secs(2_700));
        assert!(!p.allows_updates(SessionId(1)));
    }

    #[test]
    fn delayed_lag_saturates_on_early_session() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::Delayed);
        // 10s into the session — delayed shift saturates at zero.
        let ts = VirtualInstant::from_secs(10);
        assert_eq!(p.timestamp_for(SessionId(1), ts), VirtualInstant::ZERO);
    }

    #[test]
    fn forget_session_resets_to_default() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::Frozen);
        p.forget_session(SessionId(1));
        assert_eq!(p.kind_for(SessionId(1)), MarketDataTypeKind::Live);
    }

    #[test]
    fn sessions_are_independent() {
        let p = MarketDataTypePolicy::new(&MarketDataTypeConfig::default());
        p.set_for_session(SessionId(1), MarketDataType::Delayed);
        assert_eq!(p.kind_for(SessionId(1)), MarketDataTypeKind::Delayed);
        assert_eq!(p.kind_for(SessionId(2)), MarketDataTypeKind::Live);
    }
}
