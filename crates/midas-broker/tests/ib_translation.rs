//! Slice 4 unit tests: translation layer + pacing governor.
//!
//! These tests exercise only the pure-translation functions on
//! [`midas_broker::ib::translation`] and the self-contained
//! [`midas_broker::ib::pacing::PacingGovernor`]. No live IB is required.
//!
//! The translation module itself is `pub` at the crate root, so we can
//! reach its internals through `midas_broker::ib::translation::...`.
//! Most helpers are `pub(crate)`; we re-expose the ones under test
//! through a small public-shim in the library (see `lib.rs`).

use std::time::Duration;

use midas_broker::ib::{IdenticalKey, PacingConfig, PacingGovernor, PacingPolicy};
use midas_broker_core::market_data::{MarketDataError, Timeframe, WhatToShow};
use tokio::time::advance;

// ───────────────────────────────────────────────────────────────────────────
// PacingGovernor tests — covers BR-19 acceptance bullets.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn streaming_line_limit_enforced_and_released_on_drop() {
    let gov = PacingGovernor::new(PacingConfig {
        streaming_line_limit: 3,
        ..PacingConfig::default()
    });
    let a = gov.acquire_streaming_line().expect("a");
    let b = gov.acquire_streaming_line().expect("b");
    let c = gov.acquire_streaming_line().expect("c");
    assert_eq!(gov.streaming_lines(), 3);
    let e = gov.acquire_streaming_line();
    assert!(
        matches!(e, Err(MarketDataError::StreamingLineLimitExceeded)),
        "4th should be rejected, got {:?}",
        e.err()
    );
    drop(a);
    assert_eq!(gov.streaming_lines(), 2);
    // New one fits.
    let _d = gov.acquire_streaming_line().expect("after-drop");
    drop(b);
    drop(c);
}

#[tokio::test(start_paused = true)]
async fn identical_request_cooldown_blocks_second_call() {
    let gov = PacingGovernor::new(PacingConfig {
        identical_cooldown: Duration::from_secs(15),
        on_violation: PacingPolicy::Reject,
        ..PacingConfig::default()
    });
    let key = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
    gov.acquire_historical(key).await.expect("first");
    // Immediate repeat — under Reject policy, must fail.
    let e = gov.acquire_historical(key).await.expect_err("second");
    assert!(matches!(e, MarketDataError::PacingViolation(_)));
    // After the cooldown, retry is allowed.
    advance(Duration::from_secs(16)).await;
    gov.acquire_historical(key).await.expect("after-cooldown");
}

#[tokio::test(start_paused = true)]
async fn reject_policy_when_total_bucket_is_exhausted() {
    // Exhaust the total 10-min bucket in one shot: config allows 2 total,
    // per-key allows many, no identical cooldown noise.
    let gov = PacingGovernor::new(PacingConfig {
        historical_max_in_10min: 2,
        identical_burst: 10,
        identical_cooldown: Duration::from_millis(0),
        on_violation: PacingPolicy::Reject,
        ..PacingConfig::default()
    });
    let k1 = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
    let k2 = IdenticalKey::new(2, Timeframe::M1, WhatToShow::Trades, true);
    let k3 = IdenticalKey::new(3, Timeframe::M1, WhatToShow::Trades, true);
    gov.acquire_historical(k1).await.expect("k1");
    gov.acquire_historical(k2).await.expect("k2");
    let e = gov.acquire_historical(k3).await.expect_err("k3");
    assert!(matches!(e, MarketDataError::PacingViolation(_)));
}

#[tokio::test(start_paused = true)]
async fn queue_policy_waits_and_succeeds_within_limit() {
    let gov = std::sync::Arc::new(PacingGovernor::new(PacingConfig {
        identical_cooldown: Duration::from_secs(1),
        on_violation: PacingPolicy::Queue,
        max_queue_wait: Duration::from_secs(3),
        ..PacingConfig::default()
    }));
    let key = IdenticalKey::new(42, Timeframe::M5, WhatToShow::Trades, false);
    gov.acquire_historical(key).await.expect("first");
    let gov2 = gov.clone();
    let handle = tokio::spawn(async move { gov2.acquire_historical(key).await });
    // Advance past the 1s cooldown.
    advance(Duration::from_millis(1100)).await;
    handle.await.expect("join").expect("queued acquire");
}

#[tokio::test(start_paused = true)]
async fn queue_policy_rejects_when_wait_exceeds_max() {
    let gov = PacingGovernor::new(PacingConfig {
        identical_cooldown: Duration::from_secs(30),
        on_violation: PacingPolicy::Queue,
        max_queue_wait: Duration::from_millis(500),
        ..PacingConfig::default()
    });
    let key = IdenticalKey::new(7, Timeframe::M1, WhatToShow::Trades, false);
    gov.acquire_historical(key).await.expect("first");
    // Second call would require > 500ms wait; Queue falls through to
    // PacingViolation.
    let e = gov
        .acquire_historical(key)
        .await
        .expect_err("second (queued)");
    assert!(matches!(e, MarketDataError::PacingViolation(_)));
}

// ───────────────────────────────────────────────────────────────────────────
// Translation surface sanity — only the fragments that have public
// re-exports from `midas_broker::ib`. The private translators are
// exercised indirectly through the adapter's unit tests in
// `src/ib/translation.rs` (inside the crate).
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn identical_key_same_params_hash_equal() {
    let a = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
    let b = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
    assert_eq!(a, b);
}

#[test]
fn identical_key_different_use_rth_distinct() {
    let a = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
    let b = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, false);
    assert_ne!(a, b);
}
