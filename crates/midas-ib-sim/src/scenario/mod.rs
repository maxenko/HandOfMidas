//! Scenario YAML subsystem.
//!
//! - [`schema`] declares the [`Scenario`] document shape and [`Verb`]
//!   enumeration — every scripted action the DSL supports.
//! - [`loader`] parses + migrates + validates a YAML file into a typed
//!   [`Scenario`]. Errors flow via [`ScenarioError`].
//! - [`migrations`] carries the version-to-version upgrade chain.
//! - [`json_schema`] exports a JSON Schema derived from the Rust types for
//!   editor tooling.
//! - [`expr`] — parser + interpreter + `ScenarioQuery` trait for the
//!   `when:` / `assert` predicate language.
//! - [`mock_engine`] — minimum-viable stand-in that satisfies [`expr::ScenarioQuery`]
//!   and consumes [`crate::engine::types::EngineCmd`]s. Used by the runner
//!   until the real engine stages land.
//! - [`runner`] — executes a loaded scenario against an engine (mock or
//!   real) under a chosen clock.
//! - [`recording`] — persist and compare `.expected.jsonl` command logs.
//! - [`injector`] — verb → `EngineCmd` translation.

pub mod engine_adapter;
pub mod expr;
pub mod injector;
pub mod json_schema;
pub mod loader;
pub mod migrations;
pub mod mock_engine;
pub mod recording;
pub mod runner;
pub mod schema;

pub use self::loader::{load, load_from_str, ScenarioError};
pub use self::runner::{RunnerError, ScenarioResult, ScenarioRunner};
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
