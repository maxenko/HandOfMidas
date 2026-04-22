//! IB pacing governor (BR-19).
//!
//! IB enforces strict rate limits on historical and streaming requests:
//!
//! * 6 identical historical requests per 2 s (by `(con_id, bar_size,
//!   what_to_show, use_rth)`), plus a 15 s cooldown before repeating
//!   the exact same request.
//! * 60 historical requests total in any rolling 10-minute window.
//! * ~100 concurrent streaming lines (`reqMktData` + `reqRealTimeBars`
//!   + `reqTickByTickData`).
//!
//! This module implements a hand-rolled [`TokenBucket`] primitive plus a
//! [`PacingGovernor`] that combines the three limits. Hand-rolled is
//! preferred over the `governor` crate because:
//!
//! 1. The identical-key cooldown is a second-tier rule on top of the
//!    bucket ("6 in 2 s, AND not within 15 s of an identical request"),
//!    which doesn't fit the `governor` API shape cleanly.
//! 2. The implementation is < 150 LoC, fully tested with
//!    `tokio::time::pause`, and avoids one more dependency.
//!
//! Behaviour:
//!
//! * `acquire_streaming_line` bumps an `AtomicU32` and returns a guard
//!   that decrements on drop. Exceeding the limit returns
//!   `Err(MarketDataError::StreamingLineLimitExceeded)`.
//! * `acquire_historical` consults the total-10-min bucket and the
//!   per-key bucket; under [`PacingPolicy::Queue`] it sleeps up to
//!   `max_queue_wait` for a token, under [`PacingPolicy::Reject`] it
//!   returns `Err(PacingViolation)` immediately.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use midas_broker_core::market_data::{MarketDataError, Timeframe, WhatToShow};
use tokio::sync::Mutex;
use tokio::time::Instant;

// ───────────────────────────────────────────────────────────────────────────
// Config
// ───────────────────────────────────────────────────────────────────────────

/// Configuration for [`PacingGovernor`].
///
/// Defaults mirror IB's documented live-trading limits with a slight
/// safety margin on `streaming_line_limit` (IB allows ~100; we cap at
/// 95 to absorb noise).
#[derive(Debug, Clone, Copy)]
pub struct PacingConfig {
    /// Maximum historical requests per 10-minute rolling window.
    pub historical_max_in_10min: u32,
    /// Maximum identical-key historical requests per 2-second burst.
    pub identical_burst: u32,
    /// Cooldown after hitting the identical-key burst limit.
    pub identical_cooldown: Duration,
    /// Maximum concurrent streaming lines.
    pub streaming_line_limit: u32,
    /// What to do when a historical-bucket check fails.
    pub on_violation: PacingPolicy,
    /// Maximum wall-clock wait before a queued request gives up and
    /// returns `Err(PacingViolation)`. Only used under
    /// [`PacingPolicy::Queue`].
    pub max_queue_wait: Duration,
}

impl Default for PacingConfig {
    fn default() -> Self {
        Self {
            historical_max_in_10min: 60,
            identical_burst: 6,
            identical_cooldown: Duration::from_secs(15),
            streaming_line_limit: 95,
            on_violation: PacingPolicy::Queue,
            max_queue_wait: Duration::from_secs(2),
        }
    }
}

/// What to do when a historical request fails a pacing check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacingPolicy {
    /// Sleep up to `max_queue_wait` waiting for a token.
    Queue,
    /// Return [`MarketDataError::PacingViolation`] immediately.
    Reject,
}

// ───────────────────────────────────────────────────────────────────────────
// IdenticalKey
// ───────────────────────────────────────────────────────────────────────────

/// De-duplication key for IB's "identical historical request" limit.
///
/// Identity = `(con_id, bar_size, what_to_show, use_rth)`. We hash to
/// `u64` so the `DashMap` carrying per-key buckets doesn't have to
/// allocate an owning key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdenticalKey {
    con_id: i32,
    bar_size_secs: u64,
    what_to_show: WhatToShow,
    use_rth: bool,
}

impl IdenticalKey {
    /// Build a new key from the historical-request parameters.
    pub fn new(con_id: i32, bar_size: Timeframe, what_to_show: WhatToShow, use_rth: bool) -> Self {
        Self {
            con_id,
            bar_size_secs: bar_size.as_secs(),
            what_to_show,
            use_rth,
        }
    }

    fn as_u64(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

// ───────────────────────────────────────────────────────────────────────────
// TokenBucket
// ───────────────────────────────────────────────────────────────────────────

/// Basic token-bucket primitive.
///
/// Refill is linear: `capacity` tokens over `refill_period`. Tokens are
/// tracked as `f64` to support sub-integer refill rates without any
/// timer plumbing; the arithmetic is all protected by a single
/// `tokio::sync::Mutex`.
pub struct TokenBucket {
    inner: Mutex<TokenBucketInner>,
    capacity: f64,
    refill_period: Duration,
}

struct TokenBucketInner {
    tokens: f64,
    last_refill: Instant,
    last_consume: Option<Instant>,
}

impl TokenBucket {
    /// Build a bucket that starts full at `capacity` and refills over
    /// `refill_period`.
    pub fn new(capacity: u32, refill_period: Duration) -> Self {
        Self {
            inner: Mutex::new(TokenBucketInner {
                tokens: capacity as f64,
                last_refill: Instant::now(),
                last_consume: None,
            }),
            capacity: capacity as f64,
            refill_period,
        }
    }

    fn refill_rate_per_secs(&self) -> f64 {
        self.capacity / self.refill_period.as_secs_f64().max(f64::EPSILON)
    }

    /// Attempt to consume one token without blocking.
    ///
    /// Returns `Ok(())` on success, `Err(wait_hint)` on failure — the
    /// hint is how long the caller would need to sleep for one token
    /// to become available at the current refill rate.
    pub async fn try_acquire(&self) -> Result<(), Duration> {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(inner.last_refill);
        inner.tokens =
            (inner.tokens + elapsed.as_secs_f64() * self.refill_rate_per_secs()).min(self.capacity);
        inner.last_refill = now;
        if inner.tokens >= 1.0 {
            inner.tokens -= 1.0;
            inner.last_consume = Some(now);
            Ok(())
        } else {
            let rate = self.refill_rate_per_secs();
            let wait_secs = if rate > 0.0 {
                (1.0 - inner.tokens) / rate
            } else {
                self.refill_period.as_secs_f64()
            };
            Err(Duration::from_secs_f64(wait_secs.max(0.0)))
        }
    }

    /// Time of the last successful `try_acquire`. `None` if the bucket
    /// has never been consumed.
    pub async fn last_consume(&self) -> Option<Instant> {
        self.inner.lock().await.last_consume
    }

    /// Current token count (for diagnostics / tests).
    #[allow(dead_code)]
    pub async fn tokens(&self) -> f64 {
        let inner = self.inner.lock().await;
        // Do not mutate inner.last_refill here — keep this cheap.
        let elapsed = Instant::now().saturating_duration_since(inner.last_refill);
        (inner.tokens + elapsed.as_secs_f64() * self.refill_rate_per_secs()).min(self.capacity)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Streaming-line guard
// ───────────────────────────────────────────────────────────────────────────

/// RAII guard that holds a streaming-line slot.
///
/// On drop the internal counter decrements; the caller must keep the
/// guard alive for the lifetime of the upstream rust-ibapi subscription.
#[derive(Debug)]
pub struct StreamingLineGuard {
    counter: Arc<AtomicU32>,
}

impl StreamingLineGuard {
    /// Build a guard; only used by tests since the governor is the real
    /// producer.
    #[cfg(test)]
    pub fn new_for_test(counter: Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for StreamingLineGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// PacingGovernor
// ───────────────────────────────────────────────────────────────────────────

/// Central pacing governor shared across the IB market-data adapter.
pub struct PacingGovernor {
    config: PacingConfig,
    historical_total: TokenBucket,
    historical_per_key: DashMap<u64, Arc<TokenBucket>>,
    streaming_lines: Arc<AtomicU32>,
}

impl PacingGovernor {
    /// Build a governor using the supplied config.
    pub fn new(config: PacingConfig) -> Self {
        let total = TokenBucket::new(config.historical_max_in_10min, Duration::from_secs(10 * 60));
        Self {
            config,
            historical_total: total,
            historical_per_key: DashMap::new(),
            streaming_lines: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Returns the currently-active streaming line count. Useful for
    /// diagnostics and for the S5 router's debug dump.
    pub fn streaming_lines(&self) -> u32 {
        self.streaming_lines.load(Ordering::Relaxed)
    }

    /// Reserve a streaming-line slot.
    ///
    /// Returns a [`StreamingLineGuard`] whose `Drop` releases the slot;
    /// exceeding the cap returns `Err(StreamingLineLimitExceeded)`.
    pub fn acquire_streaming_line(&self) -> Result<StreamingLineGuard, MarketDataError> {
        // Increment optimistically; back off if the new value exceeds
        // the cap.
        let prev = self.streaming_lines.fetch_add(1, Ordering::Relaxed);
        if prev >= self.config.streaming_line_limit {
            self.streaming_lines.fetch_sub(1, Ordering::Relaxed);
            return Err(MarketDataError::StreamingLineLimitExceeded);
        }
        Ok(StreamingLineGuard {
            counter: Arc::clone(&self.streaming_lines),
        })
    }

    /// Reserve a historical-request slot.
    ///
    /// Consults the total 10-minute bucket, the per-key 2-second burst
    /// bucket, and the 15-second identical-cooldown. On policy
    /// [`PacingPolicy::Queue`] waits up to `max_queue_wait`; on
    /// [`PacingPolicy::Reject`] returns immediately.
    pub async fn acquire_historical(&self, key: IdenticalKey) -> Result<(), MarketDataError> {
        let policy = self.config.on_violation;
        let per_key = self.get_or_insert_per_key(key);

        // Identical-cooldown: if we have a record of a prior consume
        // less than `identical_cooldown` ago, wait or reject.
        if let Some(ts) = per_key.last_consume().await {
            let since = Instant::now().saturating_duration_since(ts);
            if since < self.config.identical_cooldown {
                let remaining = self.config.identical_cooldown - since;
                if matches!(policy, PacingPolicy::Reject) || remaining > self.config.max_queue_wait
                {
                    return Err(MarketDataError::PacingViolation(format!(
                        "identical historical request inside {}s cooldown",
                        self.config.identical_cooldown.as_secs()
                    )));
                }
                tokio::time::sleep(remaining).await;
            }
        }

        // Total 10-min bucket.
        Self::consume_bucket(&self.historical_total, policy, self.config.max_queue_wait).await?;

        // Per-key 6/2s burst bucket.
        Self::consume_bucket(&per_key, policy, self.config.max_queue_wait).await?;

        Ok(())
    }

    fn get_or_insert_per_key(&self, key: IdenticalKey) -> Arc<TokenBucket> {
        let h = key.as_u64();
        self.historical_per_key
            .entry(h)
            .or_insert_with(|| {
                Arc::new(TokenBucket::new(
                    self.config.identical_burst,
                    Duration::from_secs(2),
                ))
            })
            .clone()
    }

    async fn consume_bucket(
        bucket: &TokenBucket,
        policy: PacingPolicy,
        max_wait: Duration,
    ) -> Result<(), MarketDataError> {
        match bucket.try_acquire().await {
            Ok(()) => Ok(()),
            Err(hint) => match policy {
                PacingPolicy::Reject => Err(MarketDataError::PacingViolation(format!(
                    "pacing bucket exhausted, retry in {:?}",
                    hint
                ))),
                PacingPolicy::Queue => {
                    if hint > max_wait {
                        return Err(MarketDataError::PacingViolation(format!(
                            "pacing bucket exhausted, wait hint {:?} exceeds max {:?}",
                            hint, max_wait
                        )));
                    }
                    tokio::time::sleep(hint).await;
                    // Second attempt after the wait.
                    bucket.try_acquire().await.map_err(|h| {
                        MarketDataError::PacingViolation(format!(
                            "pacing bucket still exhausted after wait, hint {:?}",
                            h
                        ))
                    })
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    #[tokio::test(start_paused = true)]
    async fn token_bucket_accounting() {
        let b = TokenBucket::new(3, Duration::from_secs(6));
        assert!(b.try_acquire().await.is_ok());
        assert!(b.try_acquire().await.is_ok());
        assert!(b.try_acquire().await.is_ok());
        assert!(b.try_acquire().await.is_err()); // empty
        advance(Duration::from_secs(2)).await; // refill 1
        assert!(b.try_acquire().await.is_ok());
        assert!(b.try_acquire().await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn streaming_line_limit_enforced() {
        let gov = PacingGovernor::new(PacingConfig {
            streaming_line_limit: 2,
            ..PacingConfig::default()
        });
        let g1 = gov.acquire_streaming_line().expect("first");
        let g2 = gov.acquire_streaming_line().expect("second");
        let e = gov.acquire_streaming_line().expect_err("third");
        assert!(matches!(e, MarketDataError::StreamingLineLimitExceeded));
        drop(g1);
        let _g3 = gov.acquire_streaming_line().expect("after drop");
        assert_eq!(gov.streaming_lines(), 2);
        drop(g2);
        assert_eq!(gov.streaming_lines(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn identical_cooldown_rejects() {
        let gov = PacingGovernor::new(PacingConfig {
            identical_cooldown: Duration::from_secs(15),
            on_violation: PacingPolicy::Reject,
            ..PacingConfig::default()
        });
        let key = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
        gov.acquire_historical(key).await.unwrap();
        let e = gov.acquire_historical(key).await.unwrap_err();
        assert!(matches!(e, MarketDataError::PacingViolation(_)));
        // After 15s the identical-key cooldown elapses; allow retry.
        advance(Duration::from_secs(16)).await;
        gov.acquire_historical(key).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn identical_queue_waits() {
        let gov = Arc::new(PacingGovernor::new(PacingConfig {
            identical_cooldown: Duration::from_secs(1),
            on_violation: PacingPolicy::Queue,
            max_queue_wait: Duration::from_secs(5),
            ..PacingConfig::default()
        }));
        let key = IdenticalKey::new(7, Timeframe::M1, WhatToShow::Trades, false);
        gov.acquire_historical(key).await.unwrap();
        // The second call should resolve only after tokio time advances
        // past the 1s cooldown. Spawn into a task so we can drive the
        // virtual clock forward while it's pending.
        let gov2 = Arc::clone(&gov);
        let handle = tokio::spawn(async move { gov2.acquire_historical(key).await });
        advance(Duration::from_millis(1100)).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reject_policy_when_bucket_exhausted() {
        let gov = PacingGovernor::new(PacingConfig {
            historical_max_in_10min: 1,
            identical_burst: 10,
            identical_cooldown: Duration::from_millis(0),
            on_violation: PacingPolicy::Reject,
            ..PacingConfig::default()
        });
        let k1 = IdenticalKey::new(1, Timeframe::M1, WhatToShow::Trades, true);
        let k2 = IdenticalKey::new(2, Timeframe::M1, WhatToShow::Trades, true);
        gov.acquire_historical(k1).await.unwrap();
        let e = gov.acquire_historical(k2).await.unwrap_err();
        assert!(matches!(e, MarketDataError::PacingViolation(_)));
    }
}
