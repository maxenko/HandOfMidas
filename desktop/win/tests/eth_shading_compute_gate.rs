//! ETH-shading S5: sans-IO CI gate for the band-render pass.
//!
//! Loads a synthetic full-ET-day bar sequence (pre + RTH + post)
//! into a `CandleBuffer` via [`push_with_session`], runs
//! [`compute_session_bands`], and asserts the band count and band
//! bounds match the contract spelled out in
//! `plan/session-aware-charts/eth-shading.md` §F:
//!
//! - Trim-to-data semantics: each contiguous Pre/Post run emits one
//!   rectangle covering `[first.ts_open, last.ts_close]`. No tints
//!   over Regular/Closed/Holiday.
//! - Empty runs (zero pre-market bars) emit zero bands.
//! - Full ET trading day (pre + RTH + post) emits exactly two
//!   bands (one Pre, one Post).
//!
//! ## Deferred to a follow-up commit (require user hand-verification)
//!
//! - `desktop/win/.devloop/fixtures/aapl-eth-day.json` devloop replay
//!   fixture
//! - `desktop/win/tools/devloop-eth-bands.sh` smoke script
//! - Reference screenshot in `desktop/win/tests/data/screenshots/`
//! - The pixel-level SSIM diff (dev-only per plan; no CI variant)
//!
//! These are dev-loop ergonomics, not CI gates — the plan explicitly
//! calls out screenshot reference as "Hand-verified with the user
//! before check-in." This test is the actual CI-blocking gate and
//! stands on its own.

use midas_chart::compute::{
    compute_session_bands, SessionBandParams, LEGACY_BAND_POST, LEGACY_BAND_PRE,
};
use midas_core::SessionKindByte;
use midas_data::candle::CandleBuffer;

/// Build a synthetic full-ET-day M1 bar sequence: 330 pre-market
/// bars (04:00–09:30 ET, 5h30m), 390 RTH bars (09:30–16:00 ET, 6h30m),
/// 240 post-market bars (16:00–20:00 ET, 4h). All on 2026-04-01
/// (a Wednesday — a clean weekday with full coverage).
fn synthesize_eth_day() -> CandleBuffer {
    // 2026-04-01 00:00 UTC.
    const DAY_START_MS: i64 = 1_775_001_600 * 1_000;
    // Offsets in UTC seconds (matches sim layout): 09:00–14:30 pre,
    // 14:30–21:00 RTH, 21:00–25:00 post.
    const PRE_OFFSET_MS: i64 = 9 * 3_600 * 1_000;
    const RTH_OFFSET_MS: i64 = (14 * 3_600 + 30 * 60) * 1_000;
    const POST_OFFSET_MS: i64 = 21 * 3_600 * 1_000;
    const M1_MS: i64 = 60 * 1_000;

    let mut buf = CandleBuffer::with_capacity(960);

    // Pre-market: 330 bars
    for i in 0..330_i64 {
        let ts = DAY_START_MS + PRE_OFFSET_MS + i * M1_MS;
        buf.push_with_session(
            ts,
            100.0,
            100.5,
            99.5,
            100.2,
            1_000,
            SessionKindByte::PreMarket,
        );
    }

    // RTH: 390 bars
    for i in 0..390_i64 {
        let ts = DAY_START_MS + RTH_OFFSET_MS + i * M1_MS;
        buf.push_with_session(
            ts,
            100.2,
            101.0,
            99.8,
            100.8,
            10_000,
            SessionKindByte::Regular,
        );
    }

    // Post-market: 240 bars
    for i in 0..240_i64 {
        let ts = DAY_START_MS + POST_OFFSET_MS + i * M1_MS;
        buf.push_with_session(
            ts,
            100.8,
            101.2,
            100.5,
            100.9,
            500,
            SessionKindByte::PostMarket,
        );
    }

    buf
}

fn default_params() -> SessionBandParams {
    SessionBandParams {
        show_bands: true,
        bar_duration_ms: 60 * 1_000,
        pre_color: LEGACY_BAND_PRE,
        post_color: LEGACY_BAND_POST,
        separator_y: 600.0,
    }
}

/// Map index → x using a uniform "1 pixel per index" mapping. Keeps
/// the test sans-IO and free of camera/timestamp arithmetic — band
/// extents come out as integer x positions we can assert against.
fn x_at_open(i: usize) -> f32 {
    i as f32
}
fn x_at_close(i: usize) -> f32 {
    (i + 1) as f32
}

/// Full ET trading day → exactly 2 bands (one pre, one post). RTH
/// runs in between; closed/holiday don't apply.
#[test]
fn full_eth_day_emits_two_bands() {
    let buf = synthesize_eth_day();
    let params = default_params();

    let bands = compute_session_bands(&buf, 0, buf.len(), &params, &x_at_open, &x_at_close);

    assert_eq!(
        bands.len(),
        2,
        "full ET day must emit exactly 2 bands (pre + post); got {}",
        bands.len()
    );
}

/// Pre-market band covers indices [0, 330) — its right edge meets
/// the first RTH bar's open; the left edge sits at the first pre
/// bar's open.
#[test]
fn pre_market_band_bounds_match_first_run() {
    let buf = synthesize_eth_day();
    let params = default_params();

    let bands = compute_session_bands(&buf, 0, buf.len(), &params, &x_at_open, &x_at_close);

    // GridLineInstance laid out as a rectangle: start_x ↔ end_x via
    // its position fields. Just check the leftmost and rightmost
    // band X coordinates make sense — a band that "starts at index
    // 0 and ends at index 329" must cover roughly x ∈ [0, 330).
    let pre = bands.first().expect("pre band missing");
    let pre_x_min = pre.rect[0].min(pre.rect[2]);
    let pre_x_max = pre.rect[0].max(pre.rect[2]);
    assert!(
        (0.0..=1.0).contains(&pre_x_min),
        "pre band left edge should hug index 0: got {pre_x_min}",
    );
    assert!(
        (329.0..=331.0).contains(&pre_x_max),
        "pre band right edge should hug index 330: got {pre_x_max}",
    );
}

/// Post-market band covers indices [720, 960). Right edge sits at
/// the last post bar's close.
#[test]
fn post_market_band_bounds_match_last_run() {
    let buf = synthesize_eth_day();
    let params = default_params();

    let bands = compute_session_bands(&buf, 0, buf.len(), &params, &x_at_open, &x_at_close);

    let post = bands.last().expect("post band missing");
    let post_x_min = post.rect[0].min(post.rect[2]);
    let post_x_max = post.rect[0].max(post.rect[2]);
    assert!(
        (719.0..=721.0).contains(&post_x_min),
        "post band left edge should hug index 720 (RTH end + 1): got {post_x_min}",
    );
    assert!(
        (959.0..=961.0).contains(&post_x_max),
        "post band right edge should hug last index 960: got {post_x_max}",
    );
}

/// `show_bands = false` short-circuits to an empty `Vec` regardless
/// of session content. Disabled charts pay only a bool check.
#[test]
fn show_bands_off_returns_empty() {
    let buf = synthesize_eth_day();
    let params = SessionBandParams {
        show_bands: false,
        ..default_params()
    };

    let bands = compute_session_bands(&buf, 0, buf.len(), &params, &x_at_open, &x_at_close);
    assert!(bands.is_empty());
}

/// A buffer with zero pre-market bars (RTH-only weekday) emits at
/// most one band — the post-market run if any. With RTH-only data
/// we expect exactly zero bands.
#[test]
fn rth_only_buffer_emits_no_bands() {
    let mut buf = CandleBuffer::with_capacity(390);
    const DAY_START_MS: i64 = 1_775_001_600 * 1_000;
    const RTH_OFFSET_MS: i64 = (14 * 3_600 + 30 * 60) * 1_000;
    const M1_MS: i64 = 60 * 1_000;
    for i in 0..390_i64 {
        let ts = DAY_START_MS + RTH_OFFSET_MS + i * M1_MS;
        buf.push_with_session(
            ts,
            100.0,
            100.5,
            99.5,
            100.2,
            10_000,
            SessionKindByte::Regular,
        );
    }
    let params = default_params();
    let bands = compute_session_bands(&buf, 0, buf.len(), &params, &x_at_open, &x_at_close);
    assert!(
        bands.is_empty(),
        "RTH-only buffer must emit zero bands; got {} ",
        bands.len()
    );
}

/// Visible-window subset: when `[vis_start, vis_end)` only covers
/// part of the post-market run, the band still emits and is clipped
/// to that subset.
#[test]
fn visible_window_subset_clips_band() {
    let buf = synthesize_eth_day();
    let params = default_params();

    // Window covers only the last 100 post-market bars.
    let vis_start = buf.len() - 100;
    let vis_end = buf.len();
    let bands = compute_session_bands(&buf, vis_start, vis_end, &params, &x_at_open, &x_at_close);

    assert_eq!(bands.len(), 1, "single subset run → single band");
    let post = bands.first().unwrap();
    let post_x_min = post.rect[0].min(post.rect[2]);
    let post_x_max = post.rect[0].max(post.rect[2]);
    let expected_min = vis_start as f32;
    let expected_max = vis_end as f32;
    assert!(
        (expected_min - 1.0..=expected_min + 1.0).contains(&post_x_min),
        "post band left should hug vis_start={vis_start}: got {post_x_min}",
    );
    assert!(
        (expected_max - 1.0..=expected_max + 1.0).contains(&post_x_max),
        "post band right should hug vis_end={vis_end}: got {post_x_max}",
    );
}
