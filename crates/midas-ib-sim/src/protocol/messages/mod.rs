//! Client ↔ sim message enums.
//!
//! Stage 02a introduced the primitive field codec (`fields.rs`). Stage 02b
//! (this change-set) fills in the `IncomingMsg` parser for the 17-variant
//! client→sim subset defined in `plan/ib-sim/02-protocol-layer.md`. Stage 02c
//! will layer the `OutgoingMsg` encoder on top.

pub mod fields;
pub(crate) mod helpers;
pub mod incoming;
#[cfg(test)]
pub(crate) mod incoming_encoder;
#[cfg(test)]
mod incoming_tests;
pub mod outgoing;
pub mod server_versions;
pub mod shared;
pub mod types;

pub use self::incoming::{msg_id, IncomingMsg};
pub use self::outgoing::{
    ContractDetails, OpenOrderPayload, OutgoingMsg, PortfolioValuePayload, PositionPayload,
};
pub use self::types::{
    ComboLeg, ContractSpec, DeltaNeutralContract, ExecutionFilter, MarketDataType, OrderComboLeg,
    OrderSpec, TagValue,
};
