//! Parity gate: legacy chart vs. new `session_chart` stack share the
//! same TradingView-matching pre/post-market band tints.
//!
//! Per `plan/session-aware-charts/eth-shading.md` §G + S3, the legacy
//! chart carries `LEGACY_BAND_PRE` / `LEGACY_BAND_POST` constants in
//! `midas-chart::compute` (RGBA32F) and the new stack carries
//! `band_pre` / `band_post` on `midas_scene::ThemePalette`'s
//! `dark_default()` / `light_default()` (RGBA8). The two surfaces
//! paint different *extents* (trim-to-data vs. full-calendar-window)
//! by design — visual unification is a follow-up post-Phase-D — but
//! the *colours* must drift-free.
//!
//! Conversion: f32 in `[0.0, 1.0]` → u8 via `(c * 255.0).round()
//! .clamp(0, 255) as u8`. The constants in both crates are hand-set
//! to land on the same bytes after that round-trip; this test fails
//! if either side moves.
//!
//! ## CI gating note (accepted limitation)
//!
//! This test runs in the `desktop_session_chart_tests` GitHub Actions
//! job, which is currently `continue-on-error: true`. Drift surfaces
//! a yellow warning on the Actions tab but does NOT block merge until
//! the project's flip-to-required schedule lands. Mitigation falls on
//! human review: any PR touching either palette constant must touch
//! both.
//!
//! Long-term fix: extract the band tints into a shared `midas-theme`
//! crate (or fold both stacks into one). Tracked as the §G "Alternative
//! considered + rejected" follow-up — deferred until either
//! flip-to-required slips substantially or Phase D begins.

#![cfg(feature = "session_chart_tests")]

use midas_chart::compute::{LEGACY_BAND_POST, LEGACY_BAND_PRE};
use midas_scene::ThemePalette;

/// Convert one channel from f32 ([0.0, 1.0]) to u8 with the same
/// rounding rule both sides hand-set to.
fn f32_to_u8(c: f32) -> u8 {
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

fn rgba32_to_rgba8(rgba: [f32; 4]) -> [u8; 4] {
    [
        f32_to_u8(rgba[0]),
        f32_to_u8(rgba[1]),
        f32_to_u8(rgba[2]),
        f32_to_u8(rgba[3]),
    ]
}

#[test]
fn dark_palette_band_pre_matches_legacy() {
    let theme = ThemePalette::dark_default();
    let legacy_u8 = rgba32_to_rgba8(LEGACY_BAND_PRE);
    assert_eq!(
        theme.band_pre, legacy_u8,
        "dark_default().band_pre = {:?} drifted from LEGACY_BAND_PRE = {:?} \
         (RGBA32F {:?}). Either fix the constants in lockstep or extract \
         them to a shared theme crate. See \
         plan/session-aware-charts/eth-shading.md §G.",
        theme.band_pre, legacy_u8, LEGACY_BAND_PRE,
    );
}

#[test]
fn dark_palette_band_post_matches_legacy() {
    let theme = ThemePalette::dark_default();
    let legacy_u8 = rgba32_to_rgba8(LEGACY_BAND_POST);
    assert_eq!(
        theme.band_post, legacy_u8,
        "dark_default().band_post = {:?} drifted from LEGACY_BAND_POST = {:?} \
         (RGBA32F {:?}).",
        theme.band_post, legacy_u8, LEGACY_BAND_POST,
    );
}

#[test]
fn light_palette_band_pre_matches_legacy() {
    let theme = ThemePalette::light_default();
    let legacy_u8 = rgba32_to_rgba8(LEGACY_BAND_PRE);
    assert_eq!(theme.band_pre, legacy_u8);
}

#[test]
fn light_palette_band_post_matches_legacy() {
    let theme = ThemePalette::light_default();
    let legacy_u8 = rgba32_to_rgba8(LEGACY_BAND_POST);
    assert_eq!(theme.band_post, legacy_u8);
}

/// Sanity: pre and post are visually distinguishable (TradingView
/// uses two distinct tints, not one). If a future refactor
/// accidentally collapses them to the same colour the parity tests
/// above would still pass — this guard ensures distinct values.
#[test]
fn pre_and_post_are_visually_distinct() {
    assert_ne!(LEGACY_BAND_PRE, LEGACY_BAND_POST);
    let theme = ThemePalette::dark_default();
    assert_ne!(theme.band_pre, theme.band_post);
}
