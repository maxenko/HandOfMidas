//! `midas-ib-sim` — a full-parity Interactive Brokers TWS gateway simulator.
//!
//! Speaks the TWS wire protocol on TCP port 7497 (paper default), models IB's
//! pacing, line-limit, farm-status, and ordering quirks, and supports both
//! synthetic market data (Roll-GARCH-U) and Databento `.dbn` replay.
//!
//! # Stage status
//!
//! Stage 01 — **scaffold**. Every central type (`EngineCmd`, `MarketEmission`,
//! `OrderEmission`, `MarketSnapshot`, `QuirkViolation`, `EngineEvent`,
//! `EngineSnapshot`) is declared here; every public method body is `todo!()`.
//! Wave 2 agents fill in logic without touching the enum shapes.
//!
//! See `plan/ib-sim/00-index.md` for the full design.

#![deny(rust_2018_idioms)]

pub mod control;
pub mod engine;
pub mod market_data;
pub mod orders;
pub mod protocol;
pub mod quirks;
pub mod scenario;
pub mod security;
pub mod server;
pub mod session;

// ---------------------------------------------------------------------------
// Public re-exports — kept minimal so tests can embed the crate cheaply.
// ---------------------------------------------------------------------------

pub use crate::control::ControlApi;
pub use crate::engine::clock::{
    AcceleratedClock, Clock, ClockMode, RealClock, SessionAnchor, VirtualClock, VirtualInstant,
};
pub use crate::engine::types::{
    AcctValueUpdate, Bar, Bar5s, CommissionReport, ContractDetailsReq, EngineCmd, EngineEvent,
    EngineSnapshot, Execution, ExecutionFilter, HistoricalReq, MarketDataType, MarketEmission,
    MarketSnapshot, OpenOrder, OrderEmission, OrderId, OrderKind, OrderStatus, OrderStatusCode,
    PlaceOrderReq, PortfolioValueUpdate, PositionUpdate, QuirkViolation, RealTimeBarsReq, ReqId,
    SessionId, Side, SubKey, SubMode, TickAttribs, TickType, ViolationAction,
};
pub use crate::market_data::{MarketDataEngine, MarketDataMode};
pub use crate::orders::OrderSimulator;
pub use crate::quirks::QuirkGuard;
pub use crate::scenario::{
    Scenario, ScenarioError, ScenarioResult, ScenarioRunner, Verb, CURRENT_VERSION,
};
pub use crate::server::{start_sim, Sim, SimConfig, SimHandle};
pub use crate::session::{
    AnonymizeConfig, Anonymizer, CalibratedPreset, DbnEncoder, Direction, ProxyConfig, Recorder,
    ReplayMode, Replayer, TwsPcapHeader, TwsPcapReader, TwsPcapRecord, TwsPcapWriter, PCAP_MAGIC,
    PCAP_VERSION,
};
