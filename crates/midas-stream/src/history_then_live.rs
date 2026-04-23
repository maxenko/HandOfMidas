//! [`HistoryThenLive`] — chain a seekable history stream into a live
//! stream, with seam dedup.
//!
//! Shape:
//! - `history: Option<H: SeekableBarStream>` — drained until EOF.
//! - `live: L: BarStream` — takes over once history completes.
//! - `handoff_ts: Option<Timestamp>` — `ts_open` of the last bar emitted
//!   by history. Used to dedupe the seam: any live bar whose
//!   `window.open <` `handoff_ts` is dropped.
//!
//! **Seam policy**: a live bar with `window.open == handoff_ts` is
//! *preferred* over the history bar (it refreshes the seam with a
//! Partial update from the live aggregator). Only bars strictly older
//! than the seam are suppressed. This fixes bug-hunt H3, where the
//! prior `<=` semantic silently dropped fresh Partial updates when the
//! live tap opened on the same window as the last history bar.
//!
//! Seekability is *dynamic*: seeking is valid only while history has
//! not yet been drained. We therefore expose a `try_seek` method
//! instead of implementing [`SeekableBarStream`] — the trait bound is
//! too static to express "seekable sometimes."

use async_trait::async_trait;
use midas_bars::Candle;

use crate::{BarStream, BarStreamMeta, SeekableBarStream, StreamError, TimeRange, Timestamp};

/// Composite stream: drain `history`, then pass through to `live`.
/// Deduplicates at the seam by tracking the last `window.open` emitted
/// by history and suppressing any live bar at or before that timestamp.
pub struct HistoryThenLive<H, L>
where
    H: SeekableBarStream,
    L: BarStream,
{
    meta: BarStreamMeta,
    history: Option<H>,
    live: L,
    handoff_ts: Option<Timestamp>,
}

impl<H, L> HistoryThenLive<H, L>
where
    H: SeekableBarStream,
    L: BarStream,
{
    /// Build a chained stream. `meta` is usually the `history.meta()`
    /// clone; the caller owns the choice so tests can compose streams
    /// whose metas differ (e.g. fixture symbol vs. live symbol aliases).
    pub fn new(meta: BarStreamMeta, history: H, live: L) -> Self {
        Self {
            meta,
            history: Some(history),
            live,
            handoff_ts: None,
        }
    }

    /// `true` iff the history leg still has more bars to drain (or was
    /// never consumed). Once the last history bar is emitted — i.e. the
    /// inner history returns `None` from `next()` — `history` is
    /// dropped and this returns `false`.
    #[inline]
    pub fn history_active(&self) -> bool {
        self.history.is_some()
    }

    /// Last seam timestamp, if history has emitted at least one bar or
    /// been drained to EOF. Used for test assertions and debugging.
    #[inline]
    pub fn handoff_ts(&self) -> Option<Timestamp> {
        self.handoff_ts
    }

    /// Dynamic seek: delegates to the inner history stream while
    /// history is still present. Once history has been drained, returns
    /// [`StreamError::NotSeekable`].
    pub async fn try_seek(&mut self, to: Timestamp) -> Result<(), StreamError> {
        match self.history.as_mut() {
            Some(h) => h.seek(to).await,
            None => Err(StreamError::NotSeekable),
        }
    }
}

#[async_trait]
impl<H, L> BarStream for HistoryThenLive<H, L>
where
    H: SeekableBarStream,
    L: BarStream,
{
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        // Drain history first.
        if let Some(h) = self.history.as_mut() {
            match h.next().await {
                Some(c) => {
                    self.handoff_ts = Some(c.window.open);
                    return Some(c);
                }
                None => {
                    // History exhausted — drop it to prevent further
                    // reads and free resources.
                    self.history = None;
                }
            }
        }

        // Live side, with seam dedup: skip any live bars whose
        // window.open is STRICTLY before the seam. Bars at the seam
        // (same window.open as the last history bar) are forwarded so
        // downstream consumers pick up Partial refreshes — `apply()`
        // on `CandleSeries` overwrites the row in place when the
        // open-ts matches, so there's no duplication in storage.
        loop {
            let c = self.live.next().await?;
            if let Some(seam) = self.handoff_ts {
                if c.window.open < seam {
                    continue;
                }
            }
            // Advance the seam so subsequent consumers see monotonic
            // progress in `handoff_ts()`.
            self.handoff_ts = Some(c.window.open);
            return Some(c);
        }
    }

    async fn snapshot(&mut self, range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        // Dispatch rule:
        // - If history is drained, snapshot is purely a live concern —
        //   but live streams are `NotSeekable` for snapshot. Bubble up.
        // - If history is present and `range.to <= handoff_ts` (or no
        //   handoff yet, meaning all history is still ahead of us), the
        //   full range is historical.
        // - Otherwise combine: history for pre-handoff portion, live
        //   for post-handoff portion. Live snapshot will likely fail
        //   with NotSeekable — we surface that rather than silently
        //   truncating.
        let Some(h) = self.history.as_mut() else {
            return self.live.snapshot(range).await;
        };

        match self.handoff_ts {
            // Range entirely before (or at) the seam → history-only.
            Some(seam) if range.to() <= seam => h.snapshot(range).await,
            // Seam sits somewhere inside the range (or no seam yet) —
            // ask history for the full range; live snapshot is not a
            // supported historical query on non-seekable streams.
            Some(seam) => {
                let mut bars = h.snapshot(range).await?;
                // Forward-portion snapshot against the live side. If
                // live is a ChannelBarStream this returns NotSeekable;
                // we fold the history result only.
                let fwd_range = TimeRange::new(seam, range.to()).unwrap_or(TimeRange {
                    from: seam,
                    to: seam,
                });
                match self.live.snapshot(fwd_range).await {
                    Ok(mut live_bars) => {
                        // Seam policy (see module docs): strictly-before
                        // is dropped; bars AT the seam refresh the
                        // history bar. For snapshot we must replace the
                        // trailing history row when a same-open live
                        // bar is present, so pop the last history row
                        // if its open matches the incoming live seam.
                        live_bars.retain(|c| c.window.open >= seam);
                        if let Some(first_live) = live_bars.first() {
                            if first_live.window.open == seam
                                && bars.last().map(|b| b.window.open == seam).unwrap_or(false)
                            {
                                bars.pop();
                            }
                        }
                        bars.append(&mut live_bars);
                        Ok(bars)
                    }
                    Err(StreamError::NotSeekable) => Ok(bars),
                    Err(e) => Err(e),
                }
            }
            None => h.snapshot(range).await,
        }
    }
}
