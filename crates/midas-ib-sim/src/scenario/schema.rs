//! Scenario YAML schema — Rust representation of the scripted-DSL document.
//!
//! All types here are `serde`-derived and `schemars`-derived so they can
//! round-trip YAML and emit a JSON Schema for editor tooling.
//!
//! Wave-2 stages (runner, expression evaluator) build on these shapes — they
//! never re-declare fields. If a new verb is needed, add a variant to [`Verb`]
//! and bump [`CURRENT_VERSION`].
//!
//! See `plan/ib-sim/06-failure-injection.md` for the design.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current schema version. Bump on any breaking change and author a migration
/// in `scenario/migrations/`.
pub const CURRENT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Top-level scenario document
// ---------------------------------------------------------------------------

/// A fully parsed, post-migration scenario document.
///
/// The scenario is strictly declarative — it describes *what* should happen
/// and *when*; the Wave-2 runner converts it into engine commands.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Schema version. Required — loader rejects documents that omit it.
    pub version: u32,
    /// Human-readable scenario name. Used in log lines + assertion messages.
    pub name: String,
    /// Optional long-form description for the scenario header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Deterministic RNG seed — same seed + same scenario = same sim behaviour.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Clock mode selected at scenario boot.
    #[serde(default)]
    pub clock: ClockMode,
    /// Duration string, e.g. "5min", "00:05:00", or "300s". Parsed by runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Synthetic instruments the scenario will touch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<SymbolConfig>,
    /// Simulated accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<AccountConfig>,
    /// IB quirk knobs (pacing limits, line counts, …).
    #[serde(default)]
    pub quirks: QuirksConfig,
    /// Timeline of scripted events.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ScenarioEvent>,
    /// Predicates evaluated at session end.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asserts: Vec<ScenarioAssert>,
}

fn default_seed() -> u64 {
    0
}

/// Virtual / real / accelerated clock selector.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClockMode {
    /// Real wall-clock; scenario runs in actual elapsed time.
    Real,
    /// Virtual-time clock, advanced deterministically by the runner.
    #[default]
    Virtual,
    /// Accelerated — virtual time progresses at `multiplier` times real.
    /// The numeric multiplier is carried as a string argument on the
    /// scenario itself; the runner parses it.
    Accelerated,
}

// ---------------------------------------------------------------------------
// Symbols / accounts / quirks
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymbolConfig {
    pub symbol: String,
    /// Preset name (e.g. "MidCap", "Liquid"). Runner resolves to generator params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Starting mid-price. Required for synthetic generation.
    pub initial_price: f64,
    /// Optional per-symbol overrides carried as a free-form map. Schemars
    /// can't describe `serde_yaml::Value`, so we round-trip through
    /// `serde_json::Value` — YAML keys that don't fit JSON's model (e.g.
    /// non-string map keys) will fail deserialisation, which is exactly the
    /// discipline we want for a scripted DSL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub acct_code: String,
    pub starting_cash: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// IB quirk knobs. All fields optional — missing fields pick sim defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuirksConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_rate: Option<MsgRateQuirk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<LineLimitQuirk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub historical_pacing: Option<HistoricalPacingQuirk>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MsgRateQuirk {
    pub limit_per_sec: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineLimitQuirk {
    pub max_l1_lines: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalPacingQuirk {
    pub max_requests_per_10min: u32,
}

// ---------------------------------------------------------------------------
// Events — the scenario's timeline
// ---------------------------------------------------------------------------

/// One entry in `events:`. Exactly one of `at:` / `after:` / `when:` must be set
/// (the loader enforces this). The verb + typed args are carried by [`Verb`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEvent {
    /// Fixed virtual-time offset from scenario start, e.g. `00:00:05`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Relative trigger — fire `delay` after the named anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Duration from the named anchor, e.g. `45s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    /// Pattern-triggered — expression evaluated every tick; fires on first `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Expression>,
    /// Optional anchor name — other events may reference via `after:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named: Option<String>,
    /// The verb itself (carries its typed args in-variant).
    #[serde(flatten)]
    pub verb: Verb,
}

/// Top-level assertion applied at scenario end.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioAssert {
    pub cond: Expression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Expression — opaque string until Wave 2 ships the parser/interpreter.
// ---------------------------------------------------------------------------

/// Opaque wrapper around the predicate-language source. The parser lives in
/// Wave 2 (Stage 06 sub-team C); until then we carry the raw string so the
/// YAML is loss-free and round-trippable.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Expression(pub String);

impl From<String> for Expression {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Expression {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Verb — every scripted action the DSL can express.
// ---------------------------------------------------------------------------

/// Closed list of scenario verbs. Adding a variant is a breaking schema
/// change (see `CURRENT_VERSION` + write a migration).
///
/// YAML shape: `{ do: <snake_case_verb>, args: { … } }`. The `#[serde(tag, content)]`
/// attributes encode this.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "do", content = "args", rename_all = "snake_case")]
pub enum Verb {
    // ---- setup ----
    SubscribeMarketData(SubscribeMarketDataArgs),
    UnsubscribeMarketData(UnsubscribeMarketDataArgs),
    AcceptOrder(AcceptOrderArgs),
    CancelOrder(CancelOrderArgs),

    // ---- fault injection ----
    InjectDisconnect(InjectDisconnectArgs),
    InjectFarmOutage(FarmCodeArgs),
    InjectFarmRestore(FarmCodeArgs),
    InjectPacingViolation(SessionArgs),
    InjectLag(InjectLagArgs),
    InjectBadFrame(InjectBadFrameArgs),
    InjectPriceJump(InjectPriceJumpArgs),
    InjectGap(InjectGapArgs),
    InjectHalt(InjectHaltArgs),
    InjectBurst(InjectBurstArgs),
    InjectDuplicateOrderStatus(InjectDuplicateOrderStatusArgs),
    InjectSlowCommissionReport(InjectSlowCommissionReportArgs),
    InjectOutOfOrderEvents(InjectOutOfOrderEventsArgs),
    InjectDailyRestart,

    // ---- control ----
    Sleep(SleepArgs),
    SetClockMode(SetClockModeArgs),
    Include(IncludeArgs),

    // ---- assertions ----
    Assert(AssertArgs),
    AssertClientReceived(AssertClientReceivedArgs),
    AssertClientEventOrder(AssertClientEventOrderArgs),
}

// ---------------------------------------------------------------------------
// Verb arg structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubscribeMarketDataArgs {
    pub symbol: String,
    pub subscription: SubscriptionKind,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionKind {
    StreamingL1,
    TickByTickLast,
    TickByTickAllLast,
    TickByTickBidAsk,
    TickByTickMidPoint,
    RealtimeBars5s,
    Historical,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnsubscribeMarketDataArgs {
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptOrderArgs {
    pub order_kind: OrderKindArg,
    pub side: OrderSide,
    pub quantity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<EntryType>,
    /// Absolute limit price for Limit entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<f64>,
    /// Absolute stop price for Stop / StopLimit entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<f64>,
    /// Bracket TP offset in price units (+ favours direction of side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp_offset: Option<f64>,
    /// Bracket SL offset in price units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sl_offset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_ref: Option<String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderKindArg {
    /// A single parent order.
    Single,
    /// Parent + TP + SL (bracket group).
    Bracket,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelOrderArgs {
    pub order_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectDisconnectArgs {
    /// `all` closes every session; a numeric session-id closes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SessionSelector {
    /// Numeric session id assigned at accept time.
    Id(u64),
    /// Sentinel token — currently only `"all"`.
    Named(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FarmCodeArgs {
    /// IB error code — 1100, 1101, 1102, …
    pub code: i32,
    pub farms: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionArgs {
    pub session_id: SessionSelector,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectLagArgs {
    pub session_id: SessionSelector,
    /// Duration string, e.g. `5s`, `250ms`.
    pub duration: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectBadFrameArgs {
    pub session_id: SessionSelector,
    /// Hex-encoded bytes, e.g. `"hex:deadbeef"`.
    pub bytes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectPriceJumpArgs {
    pub symbol: String,
    pub magnitude_pct: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectGapArgs {
    pub symbol: String,
    pub from: f64,
    pub to: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectHaltArgs {
    pub symbol: String,
    pub duration: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectBurstArgs {
    pub symbols: Vec<String>,
    pub multiplier: f64,
    pub duration: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectDuplicateOrderStatusArgs {
    pub order_ref: String,
    /// How many times to repeat the status (default 1).
    #[serde(default = "one_u32")]
    pub count: u32,
}

fn one_u32() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectSlowCommissionReportArgs {
    pub order_ref: String,
    pub delay: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InjectOutOfOrderEventsArgs {
    /// Names of the two events to swap, in observed vs emit order.
    pub emit_first: String,
    pub emit_second: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SleepArgs {
    pub duration: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClockModeArgs {
    pub mode: ClockMode,
    /// Present only when `mode == Accelerated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IncludeArgs {
    /// Relative path to another scenario YAML.
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertArgs {
    pub cond: Expression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertClientReceivedArgs {
    pub session_id: SessionSelector,
    /// Message-type name (matches IB's outgoing IDs).
    pub message: String,
    /// Optional field-level predicate over the emitted message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Expression>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertClientEventOrderArgs {
    pub session_id: SessionSelector,
    /// Ordered list of message-type names expected in sequence.
    pub sequence: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_round_trips() {
        let e = Expression::from("orders[0].status == Filled");
        let s = serde_yaml::to_string(&e).unwrap();
        let back: Expression = serde_yaml::from_str(&s).unwrap();
        assert_eq!(e.0, back.0);
    }

    #[test]
    fn verb_variants_tag_to_snake_case() {
        // Smoke — constructing a handful of variants + serialising them is the
        // cheapest way to catch accidental shape drift.
        let v = Verb::SubscribeMarketData(SubscribeMarketDataArgs {
            symbol: "AAPL".into(),
            subscription: SubscriptionKind::StreamingL1,
        });
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("do: subscribe_market_data"));
        assert!(yaml.contains("streaming_l1"));
    }

    #[test]
    fn scenario_minimal_round_trip() {
        let sc = Scenario {
            version: 1,
            name: "min".into(),
            description: None,
            seed: 0,
            clock: ClockMode::Virtual,
            duration: None,
            symbols: vec![],
            accounts: vec![],
            quirks: QuirksConfig::default(),
            events: vec![],
            asserts: vec![],
        };
        let s = serde_yaml::to_string(&sc).unwrap();
        let back: Scenario = serde_yaml::from_str(&s).unwrap();
        assert_eq!(back.name, "min");
        assert_eq!(back.version, 1);
    }
}
