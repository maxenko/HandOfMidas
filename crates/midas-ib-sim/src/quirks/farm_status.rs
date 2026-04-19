//! Farm-status bulletins + connection-lifecycle events.
//!
//! Emits:
//! * Unsolicited "farm is OK" triplet (2104/2106/2158) ~100ms after
//!   `START_API` — required for clients like `ib_insync` to fire
//!   `connectedEvent`.
//! * 1100/1101/1102 on scenario-injected `ConnEvent`s (1101 mandates market-
//!   data re-subscription on the client side).
//! * 1300 at the scheduled daily-restart virtual time.
//! * T2-only periodic 2103/2105/2108 farm cycling (~30-minute cadence).
//!
//! All bulletins route through [`FarmBulletin`] — the engine's outbound path
//! turns each into an `OutgoingMsg::ErrMsg { req_id: -1, ... }` frame.
//!
//! # Determinism
//!
//! Timings are driven through [`Clock::now`] and scheduled via the engine's
//! [`EventScheduler`]. No `Instant::now()` leaks out of this module.

use std::time::Duration;

use crate::engine::clock::VirtualInstant;
use crate::quirks::error_codes;

/// Plan default: how long after `START_API` to wait before emitting the
/// initial farm-OK triplet. Real IB sends them "instantly" from the client's
/// perspective; 100ms models the round-trip + gateway startup latency.
pub const DEFAULT_INITIAL_BULLETIN_DELAY: Duration = Duration::from_millis(100);

/// Plan default: T2 periodic-cycling cadence.
pub const DEFAULT_FARM_CYCLE_PERIOD: Duration = Duration::from_secs(30 * 60);

/// Plan default: daily restart time — 11:45 PM ET = 03:45 UTC (during EDT).
/// Expressed as a virtual-time offset the scheduler measures from session
/// start.
///
/// The engine sets the actual restart time by passing in a `VirtualInstant`
/// at config load; this constant is only consulted when no override is given.
pub const DEFAULT_DAILY_RESTART_OFFSET: Duration = Duration::from_secs(0); // scheduler sets it

/// Which connection event triggered a farm status push.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ConnEvent {
    /// Upstream farms just disconnected — emit 1100.
    FarmLost,
    /// Farms back, streaming market-data state was lost — emit 1101 (clients
    /// re-subscribe).
    FarmRestoredNoData,
    /// Farms back, streaming state intact — emit 1102 (no client action).
    FarmRestoredData,
    /// Scheduled 11:45 PM ET daily restart — emit 1300 and close every
    /// session.
    DailyRestart,
}

/// A single bulletin to emit on every open session. The engine projects this
/// into `OutgoingMsg::ErrMsg { req_id: -1, code, message, .. }`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FarmBulletin {
    pub code: i32,
    pub message: String,
}

impl FarmBulletin {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Policy object for farm-status bulletin emission. Pure — returns work to do,
/// the engine executes it.
#[derive(Clone, Debug)]
pub struct FarmStatusEmitter {
    initial_bulletin_delay: Duration,
    farm_cycle_period: Duration,
    periodic_cycling_enabled: bool,
    /// Virtual offset from session start at which the daily restart fires.
    /// `None` disables scheduled daily restart (used in most tests).
    daily_restart_offset: Option<Duration>,
    /// Farm labels used in bulletin messages. Defaults to IB's canonical
    /// names — overridable for multi-region test scenarios.
    market_data_farm: String,
    hmds_farm: String,
    sec_def_farm: String,
}

impl Default for FarmStatusEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl FarmStatusEmitter {
    pub fn new() -> Self {
        Self {
            initial_bulletin_delay: DEFAULT_INITIAL_BULLETIN_DELAY,
            farm_cycle_period: DEFAULT_FARM_CYCLE_PERIOD,
            periodic_cycling_enabled: false,
            daily_restart_offset: None,
            market_data_farm: "usfarm".into(),
            hmds_farm: "ushmds".into(),
            sec_def_farm: "secdefil".into(),
        }
    }

    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_bulletin_delay = delay;
        self
    }

    pub fn with_periodic_cycling(mut self, on: bool, period: Duration) -> Self {
        self.periodic_cycling_enabled = on;
        self.farm_cycle_period = period;
        self
    }

    pub fn with_daily_restart_offset(mut self, offset: Option<Duration>) -> Self {
        self.daily_restart_offset = offset;
        self
    }

    pub fn initial_delay(&self) -> Duration {
        self.initial_bulletin_delay
    }

    pub fn periodic_cycling_enabled(&self) -> bool {
        self.periodic_cycling_enabled
    }

    pub fn farm_cycle_period(&self) -> Duration {
        self.farm_cycle_period
    }

    pub fn daily_restart_at(&self, session_start: VirtualInstant) -> Option<VirtualInstant> {
        self.daily_restart_offset
            .map(|d| session_start.saturating_add(d))
    }

    /// The three "farm OK" bulletins the sim sends unsolicited after
    /// `START_API`. Order matters — clients expect 2104 → 2106 → 2158.
    pub fn initial_bulletins(&self) -> Vec<FarmBulletin> {
        vec![
            FarmBulletin::new(
                error_codes::MD_FARM_OK_USFARM,
                format!(
                    "Market data farm connection is OK:{}",
                    self.market_data_farm
                ),
            ),
            FarmBulletin::new(
                error_codes::HMDS_FARM_OK_USHMDS,
                format!("HMDS data farm connection is OK:{}", self.hmds_farm),
            ),
            FarmBulletin::new(
                error_codes::SEC_DEF_FARM_OK,
                format!("Sec-def data farm connection is OK:{}", self.sec_def_farm),
            ),
        ]
    }

    /// Bulletin list produced when the engine observes a [`ConnEvent`].
    ///
    /// For `FarmLost` / `FarmRestoredNoData` / `FarmRestoredData` a single
    /// bulletin is returned. `DailyRestart` returns a `1300` message and the
    /// caller is expected to close every session after emitting.
    pub fn bulletins_for(&self, event: ConnEvent) -> Vec<FarmBulletin> {
        match event {
            ConnEvent::FarmLost => vec![FarmBulletin::new(
                error_codes::FARM_LOST,
                error_codes::message(error_codes::FARM_LOST),
            )],
            ConnEvent::FarmRestoredNoData => vec![FarmBulletin::new(
                error_codes::FARM_RESTORED_NO_DATA,
                error_codes::message(error_codes::FARM_RESTORED_NO_DATA),
            )],
            ConnEvent::FarmRestoredData => vec![FarmBulletin::new(
                error_codes::FARM_RESTORED_DATA,
                error_codes::message(error_codes::FARM_RESTORED_DATA),
            )],
            ConnEvent::DailyRestart => vec![FarmBulletin::new(
                error_codes::TWS_DAILY_RESTART,
                error_codes::message(error_codes::TWS_DAILY_RESTART),
            )],
        }
    }

    /// T2 periodic cycling — returns (down, up) bulletin pairs for the next
    /// farm cycle. Real IB alternates between the three bulletin codes; we
    /// rotate through them deterministically using `cycle_idx`.
    pub fn cycling_pair(&self, cycle_idx: u64) -> (FarmBulletin, FarmBulletin) {
        match cycle_idx % 3 {
            0 => (
                FarmBulletin::new(
                    error_codes::MD_FARM_BROKEN,
                    format!(
                        "Market data farm connection is broken:{}",
                        self.market_data_farm
                    ),
                ),
                FarmBulletin::new(
                    error_codes::MD_FARM_OK_USFARM,
                    format!(
                        "Market data farm connection is OK:{}",
                        self.market_data_farm
                    ),
                ),
            ),
            1 => (
                FarmBulletin::new(
                    error_codes::HMDS_FARM_BROKEN,
                    format!("HMDS data farm connection is broken:{}", self.hmds_farm),
                ),
                FarmBulletin::new(
                    error_codes::HMDS_FARM_OK_USHMDS,
                    format!("HMDS data farm connection is OK:{}", self.hmds_farm),
                ),
            ),
            _ => (
                FarmBulletin::new(
                    error_codes::SEC_DEF_FARM_DISCONNECTED,
                    format!(
                        "Sec-def data farm connection is disconnected:{}",
                        self.sec_def_farm
                    ),
                ),
                FarmBulletin::new(
                    error_codes::SEC_DEF_FARM_OK,
                    format!("Sec-def data farm connection is OK:{}", self.sec_def_farm),
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_bulletins_are_ordered_2104_2106_2158() {
        let em = FarmStatusEmitter::new();
        let b = em.initial_bulletins();
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].code, error_codes::MD_FARM_OK_USFARM);
        assert_eq!(b[1].code, error_codes::HMDS_FARM_OK_USHMDS);
        assert_eq!(b[2].code, error_codes::SEC_DEF_FARM_OK);
    }

    #[test]
    fn initial_bulletins_carry_farm_labels() {
        let em = FarmStatusEmitter::new();
        let b = em.initial_bulletins();
        assert!(b[0].message.ends_with(":usfarm"), "got {}", b[0].message);
        assert!(b[1].message.ends_with(":ushmds"));
        assert!(b[2].message.ends_with(":secdefil"));
    }

    #[test]
    fn conn_events_map_one_to_one_to_codes() {
        let em = FarmStatusEmitter::new();
        let assertions = [
            (ConnEvent::FarmLost, error_codes::FARM_LOST),
            (
                ConnEvent::FarmRestoredNoData,
                error_codes::FARM_RESTORED_NO_DATA,
            ),
            (ConnEvent::FarmRestoredData, error_codes::FARM_RESTORED_DATA),
            (ConnEvent::DailyRestart, error_codes::TWS_DAILY_RESTART),
        ];
        for (evt, expected) in assertions {
            let b = em.bulletins_for(evt);
            assert_eq!(b.len(), 1, "{evt:?} must produce exactly one bulletin");
            assert_eq!(b[0].code, expected, "{evt:?} → expected code {expected}");
        }
    }

    #[test]
    fn default_initial_delay_is_100ms() {
        let em = FarmStatusEmitter::new();
        assert_eq!(em.initial_delay(), Duration::from_millis(100));
    }

    #[test]
    fn periodic_cycling_is_off_by_default() {
        let em = FarmStatusEmitter::new();
        assert!(!em.periodic_cycling_enabled());
    }

    #[test]
    fn cycling_pair_rotates_through_three_farms() {
        let em = FarmStatusEmitter::new().with_periodic_cycling(true, Duration::from_secs(60));
        let (down_0, up_0) = em.cycling_pair(0);
        let (down_1, up_1) = em.cycling_pair(1);
        let (down_2, up_2) = em.cycling_pair(2);
        let (down_3, _) = em.cycling_pair(3);
        assert_eq!(down_0.code, error_codes::MD_FARM_BROKEN);
        assert_eq!(up_0.code, error_codes::MD_FARM_OK_USFARM);
        assert_eq!(down_1.code, error_codes::HMDS_FARM_BROKEN);
        assert_eq!(up_1.code, error_codes::HMDS_FARM_OK_USHMDS);
        assert_eq!(down_2.code, error_codes::SEC_DEF_FARM_DISCONNECTED);
        assert_eq!(up_2.code, error_codes::SEC_DEF_FARM_OK);
        // Wraps around.
        assert_eq!(down_3.code, error_codes::MD_FARM_BROKEN);
    }

    #[test]
    fn daily_restart_offset_defaults_off() {
        let em = FarmStatusEmitter::new();
        assert!(em.daily_restart_at(VirtualInstant::ZERO).is_none());
    }

    #[test]
    fn daily_restart_offset_projects_onto_session_start() {
        let em = FarmStatusEmitter::new().with_daily_restart_offset(Some(Duration::from_secs(60)));
        let anchor = VirtualInstant::from_secs(10);
        assert_eq!(
            em.daily_restart_at(anchor),
            Some(VirtualInstant::from_secs(70))
        );
    }
}
