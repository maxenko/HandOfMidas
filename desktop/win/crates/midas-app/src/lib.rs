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

/// Session-aware chart pipeline (S8 + Phase C). Exposed via the
/// library target so integration tests
/// (`desktop/win/tests/session_chart_e2e.rs`) can reach
/// [`session_chart::SessionChartDriver`] /
/// [`session_chart::build_scene`] / [`session_chart::translate`] and
/// exercise the full end-to-end pipeline without spinning up the
/// whole `midas-app` binary. Feature-gated on `session_chart`.
#[cfg(feature = "session_chart")]
pub mod session_chart;

/// Chart-transition parity harness (`plan/chart-transition` Slice 0).
/// Exposed at lib-root so the chart-parity-fixture integration test
/// can consume [`chart_parity::compare_images`] without pulling in
/// the bin-private `dev_harness` module. The bin's dev-harness
/// command dispatch re-exports from here.
#[cfg(feature = "dev_harness")]
pub mod chart_parity;

/// Annotation store — centralised per-symbol annotation state.
/// Exposed at lib-root so slice-4 chart-transition integration tests
/// (`desktop/win/tests/level_end_to_end.rs`) can exercise the full
/// `ToolEffect::CreateLevel` → `AnnotationStore::add_level` round-trip
/// without pulling in the binary-only `app.rs` module.
pub mod annotation_store;
