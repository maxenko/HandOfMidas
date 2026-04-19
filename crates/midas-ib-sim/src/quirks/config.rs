//! YAML-loadable configuration for every quirk module.
//!
//! Defaults reproduce T1 behaviour exactly (always-on quirks). T2 flags are
//! opt-in and default to `false` / zero so a fresh config deploys identically
//! to production IB paper for deterministic regression testing.
//!
//! # Shape
//!
//! The YAML layout mirrors `plan/ib-sim/05-quirk-modeling.md` §"Quirk
//! configuration". Each section maps 1-1 to a limiter struct:
//!
//! ```yaml
//! quirks:
//!   msg_rate:
//!     limit_per_sec: 50
//!     violation_action: disconnect
//!   line_limit:
//!     max_l1_lines: 100
//!     max_tbt: 5
//!     tbt_cooldown_sec: 15
//!   historical_pacing:
//!     window_60_10min: 60
//!     burst_6_2sec: 6
//!     identical_cooldown_sec: 15
//!     bidask_double_count: true
//!   farm_status:
//!     emit_on_connect: [2104, 2106, 2158]
//!     periodic_cycling: false
//!     initial_bulletin_delay_ms: 100
//!   fills:
//!     duplicate_order_status_rate: 0.0
//!   contract_latency_ms: [50, 200]
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::quirks::error_codes;
use crate::quirks::historical_pacing::PacingParams;

/// Top-level config — exactly the YAML shape above minus the leading
/// `quirks:` key (which the loader strips).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuirksConfig {
    pub msg_rate: MsgRateConfig,
    pub line_limit: LineLimitConfig,
    pub historical_pacing: HistoricalPacingConfig,
    pub farm_status: FarmStatusConfig,
    pub fills: FillsConfig,
    pub contract_latency_ms: ContractLatencyConfig,
    pub market_data_type: MarketDataTypeConfig,
}

impl QuirksConfig {
    /// Parse a YAML document. The caller may pass the full document (with the
    /// `quirks:` key) or just the `QuirksConfig` body — we peel the leading
    /// key if present for YAML files that namespace the sim config.
    pub fn from_yaml(src: &str) -> Result<Self, serde_yaml::Error> {
        #[derive(Deserialize)]
        struct Wrapper {
            quirks: QuirksConfig,
        }
        // Try the namespaced form first.
        if let Ok(w) = serde_yaml::from_str::<Wrapper>(src) {
            return Ok(w.quirks);
        }
        serde_yaml::from_str(src)
    }
}

// ---------------------------------------------------------------------------
// 50-msg/sec.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MsgRateConfig {
    pub limit_per_sec: u32,
    pub violation_action: MsgRateViolationAction,
}

impl Default for MsgRateConfig {
    fn default() -> Self {
        Self {
            limit_per_sec: 50,
            violation_action: MsgRateViolationAction::Disconnect,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgRateViolationAction {
    /// Emit error 100 and close the socket (default = real-IB behaviour).
    Disconnect,
    /// Log the violation but keep serving — dev-mode escape hatch.
    WarnOnly,
}

// ---------------------------------------------------------------------------
// 100-L1 / 5-TBT line cap.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineLimitConfig {
    pub max_l1_lines: u32,
    pub max_tbt: u32,
    pub tbt_cooldown_sec: u64,
}

impl Default for LineLimitConfig {
    fn default() -> Self {
        Self {
            max_l1_lines: 100,
            max_tbt: 5,
            tbt_cooldown_sec: 15,
        }
    }
}

impl LineLimitConfig {
    pub fn tbt_cooldown(&self) -> Duration {
        Duration::from_secs(self.tbt_cooldown_sec)
    }
}

// ---------------------------------------------------------------------------
// Historical pacing.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoricalPacingConfig {
    pub window_60_10min: u32,
    pub burst_6_2sec: u32,
    pub identical_cooldown_sec: u64,
    pub bidask_double_count: bool,
}

impl Default for HistoricalPacingConfig {
    fn default() -> Self {
        Self {
            window_60_10min: 60,
            burst_6_2sec: 6,
            identical_cooldown_sec: 15,
            bidask_double_count: true,
        }
    }
}

impl HistoricalPacingConfig {
    pub fn to_pacing_params(&self) -> PacingParams {
        PacingParams {
            window_limit: self.window_60_10min,
            window: Duration::from_secs(600),
            burst_limit: self.burst_6_2sec,
            burst_window: Duration::from_secs(2),
            identical_cooldown: Duration::from_secs(self.identical_cooldown_sec),
            bidask_double_count: self.bidask_double_count,
        }
    }
}

// ---------------------------------------------------------------------------
// Farm status + lifecycle.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FarmStatusConfig {
    /// Codes to emit unsolicited on session start. Defaults to the IB
    /// canonical triplet.
    pub emit_on_connect: Vec<i32>,
    /// T2: cycle farms up/down every `farm_cycle_period_sec` virtual seconds.
    pub periodic_cycling: bool,
    pub farm_cycle_period_sec: u64,
    /// Virtual delay between START_API and the first bulletin emission.
    pub initial_bulletin_delay_ms: u64,
    /// Offset from session-start at which to fire the daily-restart bulletin.
    /// `None` disables scheduled restart — tests never want it by default.
    pub daily_restart_offset_sec: Option<u64>,
}

impl Default for FarmStatusConfig {
    fn default() -> Self {
        Self {
            emit_on_connect: vec![
                error_codes::MD_FARM_OK_USFARM,
                error_codes::HMDS_FARM_OK_USHMDS,
                error_codes::SEC_DEF_FARM_OK,
            ],
            periodic_cycling: false,
            farm_cycle_period_sec: 30 * 60,
            initial_bulletin_delay_ms: 100,
            daily_restart_offset_sec: None,
        }
    }
}

// ---------------------------------------------------------------------------
// T2 — duplicate order status + fill patterns.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FillsConfig {
    /// Probability (0..=1) that an OrderStatus is emitted twice. Real IB
    /// occasionally duplicates — feature flag defaults off so regression
    /// tests see clean sequences.
    pub duplicate_order_status_rate: f64,
}

impl Default for FillsConfig {
    fn default() -> Self {
        Self {
            duplicate_order_status_rate: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// T2 — contract-details latency.
// ---------------------------------------------------------------------------

/// Tuple of `[min_ms, max_ms]` — the engine draws a uniform delay per
/// `reqContractDetails` response. Default is `[0, 0]` (the T2 quirk is off);
/// the YAML loader flips it on with a non-zero max.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractLatencyConfig {
    pub min_ms: u64,
    pub max_ms: u64,
}

impl ContractLatencyConfig {
    pub fn is_enabled(&self) -> bool {
        self.max_ms > 0
    }

    pub fn as_range_ms(&self) -> (u64, u64) {
        (self.min_ms, self.max_ms.max(self.min_ms))
    }
}

// ---------------------------------------------------------------------------
// T2 — reqMarketDataType.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketDataTypeConfig {
    /// Default data type when the client never calls `reqMarketDataType`.
    /// Matches IB — streaming live data to real accounts.
    pub default: MarketDataTypeKind,
    /// Virtual lag applied to delayed-mode ticks.
    pub delayed_lag_secs: u64,
}

impl Default for MarketDataTypeConfig {
    fn default() -> Self {
        Self {
            default: MarketDataTypeKind::Live,
            delayed_lag_secs: 15 * 60,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketDataTypeKind {
    Live,
    Frozen,
    Delayed,
    DelayedFrozen,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_t1_only() {
        let c = QuirksConfig::default();
        assert_eq!(c.msg_rate.limit_per_sec, 50);
        assert_eq!(c.line_limit.max_l1_lines, 100);
        assert_eq!(c.line_limit.max_tbt, 5);
        assert_eq!(c.line_limit.tbt_cooldown_sec, 15);
        assert_eq!(c.historical_pacing.window_60_10min, 60);
        assert_eq!(c.historical_pacing.burst_6_2sec, 6);
        assert_eq!(c.historical_pacing.identical_cooldown_sec, 15);
        assert!(c.historical_pacing.bidask_double_count);

        // T2 flags are all off.
        assert!(!c.farm_status.periodic_cycling);
        assert_eq!(c.fills.duplicate_order_status_rate, 0.0);
        assert!(!c.contract_latency_ms.is_enabled());
        assert_eq!(c.market_data_type.default, MarketDataTypeKind::Live);
    }

    #[test]
    fn yaml_round_trip_defaults() {
        let c = QuirksConfig::default();
        let yaml = serde_yaml::to_string(&c).unwrap();
        let back = QuirksConfig::from_yaml(&yaml).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn yaml_namespaced_form_loads() {
        let src = r#"
quirks:
  msg_rate:
    limit_per_sec: 100
    violation_action: warn_only
  line_limit:
    max_l1_lines: 200
    max_tbt: 10
    tbt_cooldown_sec: 30
"#;
        let c = QuirksConfig::from_yaml(src).unwrap();
        assert_eq!(c.msg_rate.limit_per_sec, 100);
        assert_eq!(
            c.msg_rate.violation_action,
            MsgRateViolationAction::WarnOnly
        );
        assert_eq!(c.line_limit.max_l1_lines, 200);
        assert_eq!(c.line_limit.max_tbt, 10);
        assert_eq!(c.line_limit.tbt_cooldown_sec, 30);
    }

    #[test]
    fn yaml_bare_form_loads() {
        let src = r#"
msg_rate:
  limit_per_sec: 25
"#;
        let c = QuirksConfig::from_yaml(src).unwrap();
        assert_eq!(c.msg_rate.limit_per_sec, 25);
    }

    #[test]
    fn historical_pacing_projects_into_params() {
        let cfg = HistoricalPacingConfig::default();
        let params = cfg.to_pacing_params();
        assert_eq!(params.window_limit, 60);
        assert_eq!(params.burst_limit, 6);
        assert_eq!(params.identical_cooldown, Duration::from_secs(15));
        assert!(params.bidask_double_count);
    }

    #[test]
    fn farm_status_default_triplet_matches_plan() {
        let cfg = FarmStatusConfig::default();
        assert_eq!(
            cfg.emit_on_connect,
            vec![
                error_codes::MD_FARM_OK_USFARM,
                error_codes::HMDS_FARM_OK_USHMDS,
                error_codes::SEC_DEF_FARM_OK,
            ]
        );
    }

    #[test]
    fn contract_latency_disabled_by_default() {
        let c = ContractLatencyConfig::default();
        assert!(!c.is_enabled());
        let (lo, hi) = c.as_range_ms();
        assert_eq!(lo, 0);
        assert_eq!(hi, 0);
    }

    #[test]
    fn contract_latency_enable_via_yaml() {
        let src = r#"
contract_latency_ms:
  min_ms: 50
  max_ms: 200
"#;
        let c = QuirksConfig::from_yaml(src).unwrap();
        assert!(c.contract_latency_ms.is_enabled());
        assert_eq!(c.contract_latency_ms.as_range_ms(), (50, 200));
    }

    #[test]
    fn market_data_type_delayed_mode_loads() {
        let src = r#"
market_data_type:
  default: delayed
  delayed_lag_secs: 600
"#;
        let c = QuirksConfig::from_yaml(src).unwrap();
        assert_eq!(c.market_data_type.default, MarketDataTypeKind::Delayed);
        assert_eq!(c.market_data_type.delayed_lag_secs, 600);
    }
}
