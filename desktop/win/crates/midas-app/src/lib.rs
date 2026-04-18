//! Public library surface for `midas-app`.
//!
//! The desktop app is primarily a binary target (`main.rs` + its
//! private modules), but a few components — notably
//! [`thumbnail_widget`] — are useful as standalone iced widgets that
//! examples and integration tests can embed without spinning up the
//! whole `midas-app` binary. This module re-exposes those components
//! through the library target so external consumers (the `examples/`
//! directory, future integration tests) can reach them.
//!
//! Binary-only modules (`app`, `watchlist`, ...) intentionally stay
//! private.

/// iced 0.14 shader widget that renders a single chart thumbnail,
/// plus its supporting snapshot / program / pipeline types.
///
/// See the module docs for the pipeline-ownership story and the
/// empty-data handling rules.
pub mod thumbnail_widget;
