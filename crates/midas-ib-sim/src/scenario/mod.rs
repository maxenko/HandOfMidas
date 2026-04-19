//! Scenario YAML subsystem.
//!
//! - [`schema`] declares the [`Scenario`] document shape and [`Verb`]
//!   enumeration — every scripted action the DSL supports.
//! - [`loader`] parses + migrates + validates a YAML file into a typed
//!   [`Scenario`]. Errors flow via [`ScenarioError`].
//! - [`migrations`] carries the version-to-version upgrade chain.
//! - [`json_schema`] exports a JSON Schema derived from the Rust types for
//!   editor tooling.
//!
//! Wave-2 runner + expression-interpreter code will live alongside these
//! modules; the types here stay stable so scenarios written today keep
//! working when the runner arrives.

pub mod injector;
pub mod json_schema;
pub mod loader;
pub mod migrations;
pub mod schema;

pub use self::loader::{load, load_from_str, ScenarioError};
pub use self::schema::{
    AcceptOrderArgs, AccountConfig, AssertArgs, AssertClientEventOrderArgs,
    AssertClientReceivedArgs, CancelOrderArgs, ClockMode, EntryType, Expression, FarmCodeArgs,
    HistoricalPacingQuirk, IncludeArgs, InjectBadFrameArgs, InjectBurstArgs, InjectDisconnectArgs,
    InjectDuplicateOrderStatusArgs, InjectGapArgs, InjectHaltArgs, InjectLagArgs,
    InjectOutOfOrderEventsArgs, InjectPriceJumpArgs, InjectSlowCommissionReportArgs,
    LineLimitQuirk, MsgRateQuirk, OrderKindArg, OrderSide, QuirksConfig, Scenario, ScenarioAssert,
    ScenarioEvent, SessionArgs, SessionSelector, SetClockModeArgs, SleepArgs,
    SubscribeMarketDataArgs, SubscriptionKind, SymbolConfig, UnsubscribeMarketDataArgs, Verb,
    CURRENT_VERSION,
};
