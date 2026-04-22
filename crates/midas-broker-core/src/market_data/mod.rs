//! Shared streaming market-data types.
//!
//! This module is the neutral vocabulary every market-data layer talks
//! in — providers (`midas-broker`), the router
//! (`midas-market-data`, slice 5), and app-side consumers all import
//! types from here. No crate below the router needs to re-invent
//! `Tick`, `Bar`, or `MarketEvent`.
//!
//! Layout:
//!
//! * [`tick`] — raw tick taxonomy ([`Tick`], [`TickKind`], [`TickType`],
//!   [`TickValue`], [`TickAttributes`], [`TickByTickKind`],
//!   [`GenericTicks`]).
//! * [`bar`] — [`Bar`], [`BarCompleteness`], plus a re-export of the
//!   crate-root [`Timeframe`].
//! * [`event`] — unified [`MarketEvent`] / [`StreamKind`] /
//!   [`EndReason`].
//! * [`farm`] — [`FarmStatus`] / [`FarmCode`].
//! * [`req_id`] — [`ReqId`] (IB wire, `i32`) and [`RouterSubId`]
//!   (router-internal, `u64`).
//! * [`error`] — [`ErrorCode`] classification and [`MarketDataError`]
//!   (thiserror enum).
//! * [`what_to_show`] — [`WhatToShow`] mirroring IB's enum.
//! * [`connection`] — [`ConnectionState`], [`Quote`], [`IbDuration`].
//! * [`contract`] — [`ContractDetails`] and a re-export of
//!   [`SecurityType`] from the crate root.
//!
//! `SymbolKey` stays at the crate root as the canonical workspace
//! type; importers can continue to use `midas_broker_core::SymbolKey`
//! or, equivalently, `midas_broker_core::market_data::SymbolKey` via
//! the re-export below.
//!
//! `TODO(S1b)`: unify with the desktop `midas-core::SymbolKey` and
//! `midas-core::Timeframe` that predate this module. Both carry richer
//! behaviour (normalisation + calendar flooring respectively) and must
//! migrate once the router lands in the desktop workspace.

pub mod bar;
pub mod connection;
pub mod contract;
pub mod error;
pub mod event;
pub mod farm;
pub mod req_id;
pub mod tick;
pub mod what_to_show;

pub use bar::{Bar, BarCompleteness, Timeframe};
pub use connection::{ConnectionState, IbDuration, Quote};
pub use contract::{ContractDetails, SecurityType};
pub use error::{ErrorCode, MarketDataError};
pub use event::{EndReason, MarketEvent, StreamKind};
pub use farm::{FarmCode, FarmStatus};
pub use req_id::{ReqId, RouterSubId};
pub use tick::{GenericTicks, Tick, TickAttributes, TickByTickKind, TickKind, TickType, TickValue};
pub use what_to_show::WhatToShow;

// Re-export `SymbolKey` so the `market_data` module is a single-stop
// import for the router vocabulary.
pub use crate::SymbolKey;
