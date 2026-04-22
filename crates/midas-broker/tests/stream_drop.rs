//! Slice 2 drop tests: every stream handle must fire its cancel
//! closure exactly once in `Drop::drop` (BR-2).
//!
//! Each test constructs the handle via its `#[doc(hidden)]`
//! `new(…)` helper (the same path the sim and IB backends use),
//! drops it, and asserts the cancel closure ran.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use midas_broker::stream::{
    HistoricalStream, HistoricalStreamEvent, RealtimeBarStream, TickStream,
};
use midas_broker_core::market_data::ReqId;
use tokio::sync::{broadcast, mpsc};

#[test]
fn tick_stream_drop_invokes_cancel() {
    let fired = Arc::new(AtomicBool::new(false));
    let fired_cb = fired.clone();
    let (_tx, rx) = broadcast::channel(16);
    let stream = TickStream::new(
        ReqId(1),
        rx,
        Arc::new(OnceLock::new()),
        Box::new(move || {
            fired_cb.store(true, Ordering::SeqCst);
        }),
    );
    assert_eq!(stream.req_id(), ReqId(1));
    drop(stream);
    assert!(
        fired.load(Ordering::SeqCst),
        "TickStream::Drop did not invoke cancel closure"
    );
}

#[test]
fn realtime_bar_stream_drop_invokes_cancel() {
    let fired = Arc::new(AtomicBool::new(false));
    let fired_cb = fired.clone();
    let (_tx, rx) = broadcast::channel(16);
    let stream = RealtimeBarStream::new(
        ReqId(7),
        rx,
        Arc::new(OnceLock::new()),
        Box::new(move || {
            fired_cb.store(true, Ordering::SeqCst);
        }),
    );
    assert_eq!(stream.req_id(), ReqId(7));
    drop(stream);
    assert!(
        fired.load(Ordering::SeqCst),
        "RealtimeBarStream::Drop did not invoke cancel closure"
    );
}

#[test]
fn historical_stream_drop_invokes_cancel() {
    let fired = Arc::new(AtomicBool::new(false));
    let fired_cb = fired.clone();
    let (_tx, rx) = mpsc::channel::<HistoricalStreamEvent>(4);
    let stream = HistoricalStream::new(
        ReqId(42),
        rx,
        Box::new(move || {
            fired_cb.store(true, Ordering::SeqCst);
        }),
    );
    assert_eq!(stream.req_id(), ReqId(42));
    drop(stream);
    assert!(
        fired.load(Ordering::SeqCst),
        "HistoricalStream::Drop did not invoke cancel closure"
    );
}

#[test]
fn tick_stream_cancel_runs_once_even_with_drop() {
    // The closure is `FnOnce` — second drop must be a no-op, not a
    // panic. This guards the `.take()` contract in Drop::drop.
    let fired_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = fired_count.clone();
    let (_tx, rx) = broadcast::channel(4);
    let stream = TickStream::new(
        ReqId(2),
        rx,
        Arc::new(OnceLock::new()),
        Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    );
    drop(stream);
    assert_eq!(fired_count.load(Ordering::SeqCst), 1);
}
