//! midas-indicators: Reusable technical analysis formulas.
//!
//! Pure math — no GPU, no framework, no chart dependencies.
//! Each struct is a streaming accumulator: feed it values one at a time,
//! read the result. Stateful so they can be driven incrementally as
//! new bars arrive.
//!
//! Depends on: nothing (leaf crate).

pub mod atr;

pub use atr::{GerchikAtr, WildersAtr};
