//! Generic subscription-stream helper shared by the chart and ticker
//! iced subscriptions (audit P1 refactor 2).
//!
//! The chart and ticker stream builders used to be ~70% textual
//! duplicates: both looked up a `SubscriptionHandle<T>` from a static
//! `DashMap`-backed registry; on miss, asked the router to
//! `subscribe_*`; installed the returned handle; then re-looked it up
//! and ran a `select!` loop that pushed each received item into a
//! [`FrameCoalescer<T>`] and flushed on a fixed cadence plus the
//! [`FrameCoalescer::should_flush_early`] size trip.
//!
//! This module extracts the common shape into [`drive_subscription`],
//! a single async function parameterised over closures for the
//! push, flush, and lag-callback steps. Each caller now owns ~25
//! lines of glue vs ~100 lines of duplicated scaffolding.
//!
//! The iced `fn`-pointer constraint on `Subscription::run_with` binds
//! only the outer builder; inside `iced::stream::channel`'s async
//! closure we are free to call a generic helper with captured
//! closures.
//!
//! The watchlist stream is structurally different (multi-symbol poll
//! on `watch::Receiver::has_changed`, not a broadcast select) and is
//! not consolidated here — see `watchlist_subscription.rs`.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use iced::futures::channel::mpsc::Sender;
use iced::futures::SinkExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use super::subscription_helpers::FrameCoalescer;

/// Outcome of the batch-flush callback. `Skip` means "no UI-visible
/// message for this batch — drop silently and keep looping"; `One`
/// schedules a single message on the iced `Sender`.
pub enum BatchEmit<Msg> {
    Skip,
    One(Msg),
}

/// Generic subscription-stream driver.
///
/// The `Item` type is what the broadcast receiver yields (one wire
/// unit — e.g. `Bar` for the chart, `Tick` for the ticker). The
/// `Acc` type is what the [`FrameCoalescer`] holds internally —
/// usually the same as `Item` for the chart path, but a projected
/// scalar (`f64`) for the ticker path that only cares about the
/// latest Last price.
///
/// * `output` — iced channel sender from `iced::stream::channel`.
/// * `rx` — broadcast receiver from a `SubscriptionHandle<Item>`'s
///   `resubscribe()`. The caller resolved the handle out of a
///   registry before calling this helper.
/// * `coalescer` — caller-configured [`FrameCoalescer<Acc>`]. Chart
///   uses `FrameCoalescer::with_capacity(8)`; ticker uses a
///   single-slot configuration with aggressive early-flush.
/// * `coalesce_window` — interval between flush ticks.
/// * `on_push` — called for every item received from the broadcast.
///   Normal callers forward to `coalescer.push(item)`; ticker-style
///   callers project to a scalar and replace the single-slot
///   contents with the latest value.
/// * `on_flush` — builds a [`BatchEmit`] from the coalescer state.
/// * `on_lag` — optional message for `RecvError::Lagged`. Chart emits
///   `ChartResync`; ticker returns `None`.
///
/// The loop exits when the broadcast closes, the output sender is
/// dropped, or `on_flush` returns a message that fails to send.
pub async fn drive_subscription<Item, Acc, Msg, OnPush, OnFlush, OnLag>(
    mut output: Sender<Msg>,
    mut rx: broadcast::Receiver<Arc<Item>>,
    mut coalescer: FrameCoalescer<Acc>,
    coalesce_window: Duration,
    mut on_push: OnPush,
    mut on_flush: OnFlush,
    mut on_lag: OnLag,
) where
    Item: Send + 'static,
    Acc: Send + 'static,
    Msg: Send + 'static,
    OnPush: FnMut(&mut FrameCoalescer<Acc>, Arc<Item>),
    OnFlush: FnMut(&mut FrameCoalescer<Acc>) -> BatchEmit<Msg>,
    OnLag: FnMut(u64) -> Option<Msg>,
{
    let mut interval = tokio::time::interval(coalesce_window);
    // First tick fires immediately; consume it so the first real
    // interval wait aligns with the caller's cadence.
    interval.tick().await;
    loop {
        tokio::select! {
            r = rx.recv() => match r {
                Ok(item) => {
                    on_push(&mut coalescer, item);
                    // M-30 size-based early flush: bursty backfills
                    // (large per-symbol lookback on reconnect) would
                    // otherwise sit in the buffer for a full window
                    // before emitting.
                    if coalescer.should_flush_early()
                        && !send_batch(&mut output, on_flush(&mut coalescer)).await
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    if let Some(msg) = on_lag(n) {
                        if output.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = interval.tick() => {
                if coalescer.has_pending()
                    && !send_batch(&mut output, on_flush(&mut coalescer)).await
                {
                    break;
                }
            }
        }
    }
}

/// Ship a [`BatchEmit`] through `output`. Returns `true` when the
/// send succeeded (or was skipped); `false` when the iced channel is
/// closed and the caller should exit the select-loop.
async fn send_batch<Msg>(output: &mut Sender<Msg>, emit: BatchEmit<Msg>) -> bool
where
    Msg: Send + 'static,
{
    match emit {
        BatchEmit::One(msg) => output.send(msg).await.is_ok(),
        BatchEmit::Skip => true,
    }
}
