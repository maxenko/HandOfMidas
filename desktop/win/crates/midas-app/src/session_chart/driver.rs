//! [`SessionChartDriver`] — owns the `BarStream` pump and the shared
//! `CandleSeries`.
//!
//! The driver's job is small and well-specified:
//!
//! 1. Hold a shared [`CandleSeries`] behind an `Arc<RwLock<_>>` so the
//!    render thread can read through a short-lived read-guard without
//!    waiting on the pump task. The pump is the ONLY writer; readers
//!    may be any number of concurrent paint passes.
//! 2. Spawn a tokio task that drains [`BarStream::next`] forever, folding
//!    each incoming [`Candle`] into the series via either
//!    [`CandleSeries::apply`] (same `window.open`) or [`CandleSeries::push`]
//!    (new bar).
//! 3. Expose a [`tokio::sync::watch`]-backed `u64` version counter — the
//!    receiver lets the widget compare against last-painted version and
//!    skip paint when the series has not advanced.
//! 4. Drop the task cleanly when the stream closes or the driver is
//!    dropped (pump task's handle is stored so Drop aborts it).
//!
//! The driver does NOT decide *how* candles are classified into sessions
//! or aligned to windows — that's the aggregator's job (S7). Here we
//! trust the stream: every [`Candle`] arriving from the stream is
//! already session-tagged, calendar-scoped, and window-aligned.
//!
//! ## Lock discipline
//!
//! - `parking_lot::RwLock` — single writer (pump), many readers
//!   (widget `paint_buckets`, tests). Writer calls `write()` only
//!   inside synchronous helpers; the guard is dropped BEFORE the next
//!   `stream.next().await`. Readers take `read()` inside paint scopes
//!   that never cross an `.await` boundary.
//! - Invariant: NEVER hold a guard across `.await`. Enforced via
//!   clippy's `await_holding_lock` when the lint is enabled.

use std::sync::Arc;

use midas_bars::{Candle, CandleSeries, Completeness};
use midas_broker_core::SymbolKey;
use midas_stream::BarStream;
use parking_lot::RwLock;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::registry::SymbolSeriesRegistry;

/// Handle to the pump task. On `drop`, the task is aborted so
/// shutdown cascades cleanly to the upstream `BarStream`.
///
/// ## Field order matters
///
/// Rust drops fields in declaration order. `_pump` is listed FIRST so
/// its `JoinHandle::Drop` aborts the pump task BEFORE the shared
/// `series` and `version_rx` drop. Today the pump only holds read /
/// write access to `series` via its own `Arc` clone, so drop order is
/// defensive more than load-bearing — but any future refactor that
/// lets the pump borrow `&self` (e.g. returning a `Stream` tied to
/// the driver's lifetime) depends on this ordering to avoid a use-
/// after-drop. App-harden L1.
pub struct SessionChartDriver {
    /// Pump task join handle. Dropped on `self` drop → `abort()` is
    /// called by `JoinHandle::Drop`. This intentionally does NOT swallow
    /// panics — if the pump panics, the test framework or tracing
    /// subscriber surfaces it.
    _pump: JoinHandle<()>,
    series: Arc<RwLock<CandleSeries>>,
    version_rx: watch::Receiver<u64>,
    /// Slice 2c: registry + the key we registered under so the Drop
    /// impl can call `deregister`. Optional — legacy callers
    /// (`spawn`) still work; slice 2c call sites use
    /// [`SessionChartDriver::spawn_with_registry`].
    registry: Option<(SymbolSeriesRegistry, SymbolKey)>,
}

/// Receiver side of the driver's version counter. Each bump on
/// [`CandleSeries`] triggers one `watch::Sender::send_replace`, which
/// wakes every receiver at most once per tick.
pub type VersionReceiver = watch::Receiver<u64>;

/// Reasons the driver spawn can fail. Currently none — the driver
/// always succeeds — but the error type is here so future refinements
/// (calendar validation on a non-resolver codepath, etc.) can extend
/// without a breaking-change.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// Reserved for future use. Never returned by the current impl.
    #[error("driver construction failed: {0}")]
    Construction(String),
}

impl SessionChartDriver {
    /// Spawn the pump task over `stream`. The returned driver shares
    /// `series` with the widget; caller holds its own `Arc` to the
    /// series to read primitives off the hot path.
    ///
    /// The `series` Arc MUST be exclusive to this driver — the pump
    /// task is the ONLY writer. Readers on other threads should take
    /// their own clone of the Arc and `try_lock` or `blocking_lock`
    /// during paint.
    pub fn spawn<S>(series: Arc<RwLock<CandleSeries>>, stream: S) -> Self
    where
        S: BarStream + Send + 'static,
    {
        Self::spawn_inner(series, stream, None)
    }

    /// Slice 2c: spawn + register the shared series in a
    /// [`SymbolSeriesRegistry`] so the app's `QuoteBatch` handler can
    /// find it from a `SymbolKey`. The driver's `Drop` deregisters
    /// automatically.
    pub fn spawn_with_registry<S>(
        series: Arc<RwLock<CandleSeries>>,
        stream: S,
        registry: SymbolSeriesRegistry,
        key: SymbolKey,
    ) -> Self
    where
        S: BarStream + Send + 'static,
    {
        registry.register(key.clone(), &series);
        Self::spawn_inner(series, stream, Some((registry, key)))
    }

    fn spawn_inner<S>(
        series: Arc<RwLock<CandleSeries>>,
        mut stream: S,
        registry: Option<(SymbolSeriesRegistry, SymbolKey)>,
    ) -> Self
    where
        S: BarStream + Send + 'static,
    {
        let (version_tx, version_rx) = watch::channel::<u64>(0);
        let series_for_pump = Arc::clone(&series);
        let pump = tokio::spawn(async move {
            // `tracing::debug!` for observability — the e2e test asserts
            // on series len/version so we don't need a test hook here.
            tracing::debug!(
                "session_chart: pump task starting on symbol {:?} period {:?}",
                stream.meta().symbol,
                stream.meta().period,
            );
            while let Some(candle) = stream.next().await {
                // `parking_lot::RwLock::write` is a non-async, fast
                // spin-then-park path. The critical section here does
                // NOT await — we mutate the series and drop the guard
                // before the next `stream.next().await`. Invariant:
                // NEVER hold this guard across an `.await`. Enforced by
                // clippy::await_holding_lock when the lint is enabled.
                let v = {
                    let mut s = series_for_pump.write();
                    apply_candle(&mut s, candle);
                    s.version()
                };
                // `send_replace` wakes all watchers at most once per
                // version. Dropping on failure (no receivers) is fine —
                // the driver outlives receivers by design, so send_replace
                // failing simply means nobody is watching.
                let _ = version_tx.send(v);
            }
            tracing::debug!("session_chart: pump task exiting (stream closed)");
        });
        Self {
            _pump: pump,
            series,
            version_rx,
            registry,
        }
    }

    /// Shared handle to the `CandleSeries` the pump is writing into.
    /// Consumers (widget paint, tests) clone the returned `Arc` and
    /// take a short-lived `read()` guard when they need to read.
    pub fn series(&self) -> Arc<RwLock<CandleSeries>> {
        Arc::clone(&self.series)
    }

    /// Fresh receiver for the version counter. Multiple receivers are
    /// supported — `watch::Receiver::clone` is cheap.
    pub fn version_receiver(&self) -> VersionReceiver {
        self.version_rx.clone()
    }

    /// Current series version without awaiting a change. Useful for
    /// tests that want a synchronous snapshot.
    pub fn current_version(&self) -> u64 {
        *self.version_rx.borrow()
    }
}

impl Drop for SessionChartDriver {
    fn drop(&mut self) {
        if let Some((reg, key)) = self.registry.take() {
            reg.deregister(&key);
        }
        // Pump task is aborted by `JoinHandle::Drop` (field-order
        // comment above).
    }
}

/// Fold an arriving [`Candle`] into the series. Partial candles use
/// `apply` (overwrite-last-on-same-open-ts); completed candles fall
/// through to `push` unless they match the last open-ts (which would
/// mean an in-progress bar just went to `Completed`, so `apply`
/// continues to be correct).
fn apply_candle(series: &mut CandleSeries, candle: Candle) {
    match candle.completeness {
        Completeness::Partial => series.apply(candle),
        Completeness::Completed => {
            // The aggregator emits `Completed` at rollover with the SAME
            // `window.open` as the last `Partial`, then a fresh `Partial`
            // for the new window. `apply` handles both — overwrite if
            // the open-ts matches, else push.
            series.apply(candle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use midas_bars::{BarPeriod, Ohlcv, Symbol};
    use midas_calendar::{crypto_spot, Timestamp};
    use midas_stream::{BarStreamMeta, StreamError, TimeRange};
    use tokio::sync::mpsc;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_crypto_candle(ts: Timestamp, price: f64, completeness: Completeness) -> Candle {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(price, price + 10.0, price - 10.0, price + 5.0, 1, 1, None).unwrap();
        Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            completeness,
        )
        .unwrap()
    }

    /// Minimal `BarStream` driven by an mpsc receiver — unit tests
    /// push candles into the sender, stream yields them out.
    struct MockStream {
        rx: mpsc::Receiver<Candle>,
        meta: BarStreamMeta,
    }

    impl MockStream {
        fn new() -> (mpsc::Sender<Candle>, Self) {
            let cal = crypto_spot();
            let sym = Symbol::new("BTC-USD", cal.id());
            let (tx, rx) = mpsc::channel(32);
            let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
            (tx, Self { rx, meta })
        }
    }

    #[async_trait]
    impl BarStream for MockStream {
        fn meta(&self) -> &BarStreamMeta {
            &self.meta
        }
        async fn next(&mut self) -> Option<Candle> {
            self.rx.recv().await
        }
        async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
            Err(StreamError::NotSeekable)
        }
    }

    fn fresh_series() -> Arc<RwLock<CandleSeries>> {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    #[tokio::test]
    async fn pump_five_completed_candles_reaches_series() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);
        let mut rx = driver.version_receiver();

        let start = utc(2024, 3, 1, 0, 0);
        for i in 0..5 {
            let ts = start + chrono::Duration::minutes(i);
            tx.send(mk_crypto_candle(
                ts,
                50_000.0 + i as f64,
                Completeness::Completed,
            ))
            .await
            .unwrap();
        }
        drop(tx); // close the stream so the pump exits

        // Wait for the version to reach 5 — each candle bumps once.
        while *rx.borrow_and_update() < 5 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let s = series.read();
        assert_eq!(s.len(), 5);
    }

    #[tokio::test]
    async fn version_counter_ticks_on_each_candle() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);
        let mut rx = driver.version_receiver();

        let start = utc(2024, 3, 1, 0, 0);
        let mut last_version = *rx.borrow_and_update();
        for i in 0..3 {
            let ts = start + chrono::Duration::minutes(i);
            tx.send(mk_crypto_candle(ts, 50_000.0, Completeness::Completed))
                .await
                .unwrap();
            // Wait for a new version.
            rx.changed().await.unwrap();
            let v = *rx.borrow_and_update();
            assert!(
                v > last_version,
                "version must advance: {last_version} -> {v}"
            );
            last_version = v;
        }
    }

    #[tokio::test]
    async fn pump_exits_when_stream_closes() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

        // Send one candle so the pump is awake.
        tx.send(mk_crypto_candle(
            utc(2024, 3, 1, 0, 0),
            50_000.0,
            Completeness::Completed,
        ))
        .await
        .unwrap();

        // Close the stream.
        drop(tx);

        // Give the pump a beat to observe the close.
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            driver.version_receiver().changed(),
        )
        .await;

        // Now drop the driver; this should abort the (already-exited)
        // pump task without panicking.
        drop(driver);
        // Series lock still works — i.e. the pump's Arc to the series
        // has been released.
        let s = series.read();
        assert_eq!(s.len(), 1);
    }

    #[tokio::test]
    async fn partial_candle_is_applied_via_apply() {
        // Two candles with the same window.open — first Partial,
        // then refined Partial. apply() should overwrite, not push.
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);
        let mut rx = driver.version_receiver();

        let ts = utc(2024, 3, 1, 0, 0);
        tx.send(mk_crypto_candle(ts, 50_000.0, Completeness::Partial))
            .await
            .unwrap();
        rx.changed().await.unwrap();
        tx.send(mk_crypto_candle(ts, 50_100.0, Completeness::Partial))
            .await
            .unwrap();
        rx.changed().await.unwrap();

        let s = series.read();
        // One row because both had the same window.open.
        assert_eq!(s.len(), 1);
    }

    #[tokio::test]
    async fn current_version_is_synchronous() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let driver = SessionChartDriver::spawn(series, stream);
        assert_eq!(driver.current_version(), 0);

        tx.send(mk_crypto_candle(
            utc(2024, 3, 1, 0, 0),
            50_000.0,
            Completeness::Completed,
        ))
        .await
        .unwrap();
        // Give the pump a moment to process.
        let mut rx = driver.version_receiver();
        rx.changed().await.unwrap();
        assert!(driver.current_version() >= 1);
    }

    // ── Slice 2c: registry integration ──────────────────────────────

    fn key(s: &str) -> SymbolKey {
        SymbolKey {
            symbol: s.to_string(),
            contract_id: 0,
        }
    }

    #[tokio::test]
    async fn spawn_with_registry_registers_shared_arc() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let registry = SymbolSeriesRegistry::new();
        let k = key("BTC-USD");
        let driver = SessionChartDriver::spawn_with_registry(
            Arc::clone(&series),
            stream,
            registry.clone(),
            k.clone(),
        );
        // Registry now holds the same Arc via a weak ref.
        let got = registry.get(&k).expect("registered");
        assert!(Arc::ptr_eq(&got, &series));
        drop(tx);
        drop(driver);
    }

    #[tokio::test]
    async fn dropping_driver_deregisters_registry_entry() {
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let registry = SymbolSeriesRegistry::new();
        let k = key("BTC-USD");
        let driver = SessionChartDriver::spawn_with_registry(
            Arc::clone(&series),
            stream,
            registry.clone(),
            k.clone(),
        );
        assert!(registry.get(&k).is_some());
        drop(tx);
        drop(driver);
        // With the driver gone, the registry explicit-deregister path
        // has also fired.
        assert!(
            registry.get(&k).is_none(),
            "driver drop must deregister registry entry"
        );
    }

    #[tokio::test]
    async fn quote_fold_through_registry_reaches_shared_series() {
        // End-to-end: register the series → fold a tick via the
        // registry's Arc → observe the close on the shared series.
        let series = fresh_series();
        let (tx, stream) = MockStream::new();
        let registry = SymbolSeriesRegistry::new();
        let k = key("BTC-USD");
        let driver = SessionChartDriver::spawn_with_registry(
            Arc::clone(&series),
            stream,
            registry.clone(),
            k.clone(),
        );

        // Seed one candle so update_last_price has a target row.
        tx.send(mk_crypto_candle(
            utc(2024, 3, 1, 0, 0),
            50_000.0,
            Completeness::Completed,
        ))
        .await
        .unwrap();
        let mut rx = driver.version_receiver();
        rx.changed().await.unwrap();

        // Fold a tick through the registry's shared Arc.
        let handle = registry.get(&k).unwrap();
        {
            let mut g = handle.write();
            g.update_last_price(50_100.0);
        }
        let row = series.read().at(0).map(|r| r.close()).unwrap();
        assert!((row - 50_100.0).abs() < 1e-3);
    }
}
