//! `Filtered<S, P>` — predicate combinator that skips rejected candles
//! on both `next()` and `snapshot()`.
//!
//! Ships two concrete policies:
//! - [`EhFilter`] — equities extended-hours policy: allow/deny
//!   `PreMarket` and `PostMarket`; `Regular`, `Break`, `Overnight` are
//!   always allowed; `Closed` is always dropped.
//! - [`SessionKindFilter`] — generic allow-list of [`SessionKind`]s.

use async_trait::async_trait;
use midas_bars::Candle;
use midas_calendar::SessionKind;
use smallvec::SmallVec;

use crate::{BarStream, BarStreamMeta, StreamError, TimeRange};

// ---------------------------------------------------------------------------
// FilterPolicy
// ---------------------------------------------------------------------------

/// Per-candle accept/reject predicate. Stateless by convention so a
/// filter can be cloned cheaply and applied in parallel to `snapshot`
/// results and `next()` streams.
pub trait FilterPolicy: Send + Sync + 'static {
    fn accept(&self, candle: &Candle) -> bool;
}

// ---------------------------------------------------------------------------
// EhFilter
// ---------------------------------------------------------------------------

/// Equities extended-hours policy. `allow_pre = false` drops
/// `SessionKind::PreMarket` candles; `allow_post = false` drops
/// `SessionKind::PostMarket`. `Regular`, `Break`, and `Overnight`
/// always pass. `Closed` always drops — a bar tagged Closed is malformed
/// by construction but we defend regardless.
#[derive(Copy, Clone, Debug)]
pub struct EhFilter {
    pub allow_pre: bool,
    pub allow_post: bool,
}

impl EhFilter {
    /// Only RTH (and future intra-session breaks / overnight).
    pub const RTH_ONLY: Self = Self {
        allow_pre: false,
        allow_post: false,
    };

    /// Everything — equivalent to the identity filter for equities.
    pub const ALL: Self = Self {
        allow_pre: true,
        allow_post: true,
    };
}

impl FilterPolicy for EhFilter {
    fn accept(&self, candle: &Candle) -> bool {
        match candle.session.kind() {
            SessionKind::Regular | SessionKind::Break | SessionKind::Overnight => true,
            SessionKind::PreMarket => self.allow_pre,
            SessionKind::PostMarket => self.allow_post,
            SessionKind::Closed => false,
            // `SessionKind` is `#[non_exhaustive]` so the compiler
            // demands a wildcard. Treat any future variant
            // conservatively: drop it. Callers that want richer behaviour
            // for a new kind can write their own `FilterPolicy`.
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionKindFilter
// ---------------------------------------------------------------------------

/// Generic allow-list over [`SessionKind`]. `SmallVec<[_; 4]>` keeps the
/// typical policy inline (most cases list at most three kinds).
#[derive(Clone, Debug)]
pub struct SessionKindFilter(pub SmallVec<[SessionKind; 4]>);

impl SessionKindFilter {
    /// Build a policy from an iterator of `SessionKind`s.
    pub fn new<I: IntoIterator<Item = SessionKind>>(kinds: I) -> Self {
        Self(kinds.into_iter().collect())
    }
}

impl FilterPolicy for SessionKindFilter {
    fn accept(&self, candle: &Candle) -> bool {
        self.0.contains(&candle.session.kind())
    }
}

// ---------------------------------------------------------------------------
// Filtered combinator
// ---------------------------------------------------------------------------

/// Wraps a stream with a policy. Proxies `meta()`, skips rejected
/// candles on `next()`, and filters `snapshot()` results post-collection.
pub struct Filtered<S: BarStream, P: FilterPolicy> {
    inner: S,
    policy: P,
}

impl<S: BarStream, P: FilterPolicy> Filtered<S, P> {
    pub fn new(inner: S, policy: P) -> Self {
        Self { inner, policy }
    }

    #[inline]
    pub fn policy(&self) -> &P {
        &self.policy
    }

    #[inline]
    pub fn inner(&self) -> &S {
        &self.inner
    }

    #[inline]
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

#[async_trait]
impl<S, P> BarStream for Filtered<S, P>
where
    S: BarStream,
    P: FilterPolicy,
{
    fn meta(&self) -> &BarStreamMeta {
        self.inner.meta()
    }

    async fn next(&mut self) -> Option<Candle> {
        loop {
            let c = self.inner.next().await?;
            if self.policy.accept(&c) {
                return Some(c);
            }
        }
    }

    async fn snapshot(&mut self, range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        let mut bars = self.inner.snapshot(range).await?;
        bars.retain(|c| self.policy.accept(c));
        Ok(bars)
    }
}
