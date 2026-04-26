//! End-to-end integration test for S8 (session-aware-charts Phase B).
//!
//! Exercises the full new-stack pipeline:
//!
//! - A minimal in-memory [`midas_broker_core::provider::MarketDataSource`]
//!   (implemented below as `SyntheticBtcSource`) emits 100 synthetic
//!   BTC trade ticks via its tick broadcast channel.
//! - `midas_bars_adapter::subscribe_aggregated_bars` wires the ticks
//!   through a [`SessionedBarAggregator`] into a
//!   [`SessionedBarStream`].
//! - A [`midas_app::session_chart::SessionChartDriver`] pumps the
//!   stream into a shared `CandleSeries`.
//! - We assert:
//!   - The series accumulates at least N candles (we bound the wait on
//!     a version-counter watch; 5 s timeout for safety).
//!   - The series' calendar is `crypto_spot`.
//!   - Every row is tagged with a `Regular` session.
//! - We then hand-paint a [`midas_app::session_chart::SessionChart`]
//!   widget and assert the `RenderBuckets` emission is non-empty on
//!   the candle bucket — proving
//!   `ScenePrimitives → RenderBuckets` survives the full chain.
//!
//! Gated on the `session_chart_tests` feature so routine
//! `cargo test -p midas-workspace` remains unchanged.
//!
//! Run with:
//!   cargo test -p midas-workspace --features session_chart_tests \
//!     --test session_chart_e2e

#![cfg(feature = "session_chart_tests")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use midas_app::session_chart::{
    build_scene, translate, SceneConfig, SceneLayers, SessionChartDriver,
};
use midas_axis::{ContinuousAxis, PriceRange, Viewport};
use midas_bars::{BarPeriod, CandleSeries, Symbol};
use midas_bars_adapter::{subscribe_aggregated_bars, StaticSymbolResolver};
use midas_broker_core::market_data::{
    ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration, MarketDataError, ReqId,
    SecurityType, Tick, TickAttributes, TickByTickKind, TickKind, TickType, TickValue, Timeframe,
    WhatToShow,
};
use midas_broker_core::provider::{
    HistoricalBarsResult, HistoricalStream, MarketDataSource, RealtimeBarStream, TickStream,
};
use midas_broker_core::SymbolKey;

// `MarketEvent` and `SessionKind` are in scope via the module imports
// above; the test body uses them but clippy flags the explicit imports
// as unused since they are reached through `midas_calendar::SessionKind`
// elsewhere. Keeping the direct paths avoids the churn.
use midas_calendar::{crypto_spot, SessionKind};
use midas_clock::{Clock, SystemClock};
use midas_scene::{InteractionState, ScenePrimitives, ThemePalette};
use tokio::sync::{broadcast, watch};

/// Synthetic `MarketDataSource`. Only `subscribe_ticks` is implemented —
/// every other method errors or returns a no-op channel. Designed to
/// feed the session-aware aggregator through its single real input
/// path.
struct SyntheticBtcSource {
    /// Broadcast channel the test drives. The aggregator's tick pump
    /// subscribes here in `subscribe_ticks`.
    tick_tx: broadcast::Sender<Arc<Tick>>,
    /// Shared `subscribe_ticks` sub-id — broker-core expects a fresh
    /// `ReqId` per subscribe but the aggregator never uses the ReqId
    /// beyond logging, so a single shared counter suffices here.
    next_req_id: std::sync::atomic::AtomicI32,
    /// Set to `true` on the first `subscribe_ticks` call so the
    /// synthetic-emitter task can tell when to start pushing ticks.
    subscribed: Arc<AtomicBool>,
    /// Connection state watch — stays in `Ready` for the whole test.
    conn_state_tx: watch::Sender<ConnectionState>,
    /// Farm status broadcast — unused by the test but mandated by the
    /// trait.
    farm_tx: broadcast::Sender<FarmStatus>,
}

impl SyntheticBtcSource {
    fn new() -> Arc<Self> {
        let (tick_tx, _keep_alive_rx) = broadcast::channel::<Arc<Tick>>(1024);
        let (conn_state_tx, _) = watch::channel::<ConnectionState>(ConnectionState::Ready);
        let (farm_tx, _) = broadcast::channel::<FarmStatus>(16);
        Arc::new(Self {
            tick_tx,
            next_req_id: std::sync::atomic::AtomicI32::new(1),
            subscribed: Arc::new(AtomicBool::new(false)),
            conn_state_tx,
            farm_tx,
        })
    }

    /// Spawn a background task that emits `count` synthetic BTC trade
    /// ticks spaced `spacing` apart starting at `start_ts`. Waits for
    /// the aggregator to subscribe before emitting.
    fn spawn_emitter(
        self: &Arc<Self>,
        symbol_key: SymbolKey,
        start_ts: chrono::DateTime<Utc>,
        spacing: chrono::Duration,
        count: usize,
    ) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            // Wait for subscribe.
            while !this.subscribed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            for i in 0..count {
                let ts = start_ts + spacing * i as i32;
                let price = 50_000.0 + (i as f64) * 5.0;
                let tick = Tick {
                    symbol: symbol_key.clone(),
                    req_id: ReqId(1),
                    kind: TickKind::PriceSize,
                    tick_type: TickType::Last,
                    value: TickValue::PriceSize { price, size: 10 },
                    attrs: TickAttributes::default(),
                    ts,
                };
                let _ = this.tick_tx.send(Arc::new(tick));
            }
        })
    }
}

#[async_trait]
impl MarketDataSource for SyntheticBtcSource {
    async fn subscribe_ticks(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        let rx = self.tick_tx.subscribe();
        let req = ReqId(
            self.next_req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        self.subscribed.store(true, Ordering::SeqCst);
        // Synthetic source has nothing to cancel upstream; the cancel
        // closure is a no-op.
        Ok(TickStream::new(
            req,
            rx,
            Arc::new(std::sync::OnceLock::new()),
            Box::new(|| {}),
        ))
    }

    async fn subscribe_tick_by_tick(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn subscribe_realtime_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _what: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn historical_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _end: chrono::DateTime<Utc>,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn historical_stream(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn resolve_contract(
        &self,
        _symbol: &SymbolKey,
        _sec_type: SecurityType,
        _exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.conn_state_tx.subscribe()
    }

    fn name(&self) -> &str {
        "synthetic-btc"
    }
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_m1_end_to_end_pipeline_produces_candles() {
    let source = SyntheticBtcSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    // Wire the aggregator via the public adapter helper.
    let stream = subscribe_aggregated_bars(
        source_as_trait,
        &resolver,
        "BTC-USD",
        BarPeriod::m1(),
        clock,
    )
    .await
    .expect("subscribe_aggregated_bars should succeed for BTC-USD");

    // Capture meta BEFORE spawning the driver; the driver owns the
    // stream and `meta()` is no longer reachable through the driver
    // itself.
    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    assert_eq!(meta.calendar.id(), crypto_spot().id());
    assert_eq!(meta.period, BarPeriod::m1());

    // Build the shared series behind a parking_lot RwLock — matches the
    // production widget's ownership model (single writer = driver pump,
    // many concurrent readers = paint passes).
    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    // Spawn the driver pump.
    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    // Spawn 100 synthetic ticks spanning ~15 minutes (so we cross
    // several 1-minute windows and the aggregator emits multiple
    // completed candles).
    let start = utc(2024, 3, 1, 0, 0, 0);
    let symbol_key = SymbolKey {
        contract_id: 1_000_000_001,
        symbol: "BTC-USD".to_string(),
    };
    let emitter = source.spawn_emitter(
        symbol_key,
        start,
        chrono::Duration::seconds(9), // 9s spacing → 100 ticks ≈ 15 m
        100,
    );

    // Wait for at least 10 rows in the series (not just version ticks —
    // `Partial` emits bump the version between rollovers but don't add
    // rows). Poll every 25 ms; bail at 5 s.
    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 10 {
                return;
            }
            // Either wait on a new version, or fall through on a 25 ms
            // poll timer — the extra wakeup bounds total latency when
            // the aggregator is rate-limiting partials heavily.
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(
        wait.is_ok(),
        "pipeline should produce >=10 candle rows within 5s"
    );

    // Inspect the series.
    {
        let s = series.read();
        assert!(s.len() >= 10, "expected >=10 candles, got {}", s.len());
        assert_eq!(s.calendar(), crypto_spot().id());
        assert_eq!(s.period(), BarPeriod::m1());
        // Every bar in the 00:00..00:15 UTC window is a Regular crypto
        // session bar.
        for row in s.iter() {
            assert_eq!(row.session_kind(), SessionKind::Regular);
        }
    }

    // Emitter task has already completed by now; join just to be tidy.
    let _ = emitter.await;

    // The post-R1 builder shares the driver's `Arc<RwLock<CandleSeries>>`
    // directly with every layer — no per-frame deep copy. Drop the
    // driver so the pump task exits and no further candles can land.
    drop(driver);
    // Give the aborted pump a moment to release its Arc on the series.
    // `JoinHandle::abort` is async; the pump may still be mid-loop.
    tokio::task::yield_now().await;

    // Build the scene end-to-end.
    let axis_start = start;
    let axis_end = start + chrono::Duration::hours(1);
    let axis = ContinuousAxis::new(axis_start, axis_end, 1200.0).unwrap();
    let pr = PriceRange::new(49_900.0, 50_700.0).unwrap();
    let vp = Viewport::new(1200.0, 400.0);
    let palette = ThemePalette::dark_default();
    let interaction = InteractionState::new();
    let scene = build_scene(SceneConfig {
        series: Arc::clone(&series),
        axis,
        price_range: pr,
        viewport: vp,
        palette,
        calendar: crypto_spot(),
        interaction: &interaction,
        layers: SceneLayers::all_on(),
        time_window: (axis_start, axis_end),
        series_changed: true,
        volume_profile_config: midas_scene::VolumeProfileConfig::default(),
        volume_profile_range: 0..0,
    })
    .expect("build_scene should succeed on a populated crypto series");

    let mut primitives = ScenePrimitives::default();
    scene.paint(&mut primitives);
    // Take the series row count AT the paint moment — the shared
    // `Arc<RwLock<_>>` means the series and the paint output always
    // agree, provided no concurrent writer is alive (driver is
    // already dropped above).
    let snapshot_len = series.read().len();
    assert!(
        snapshot_len >= 10,
        "expected >=10 candles at paint time, got {snapshot_len}"
    );
    assert_eq!(
        primitives.candles.len(),
        snapshot_len,
        "scene.paint should emit one CandleInstance per series row"
    );

    // And the bridge translates cleanly.
    let buckets = translate(&primitives);
    assert_eq!(
        buckets.candles.len(),
        snapshot_len,
        "translate should 1:1 map scene candles to legacy CandleInstance"
    );
    assert!(
        !buckets.lines.is_empty(),
        "grid layer should emit lines in the e2e chain"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_series_still_emits_grid_and_axis() {
    // The widget must still produce a paintable scene when the driver
    // hasn't had any ticks yet.
    let source = SyntheticBtcSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream = subscribe_aggregated_bars(
        source_as_trait,
        &resolver,
        "BTC-USD",
        BarPeriod::m1(),
        clock,
    )
    .await
    .expect("subscribe should succeed");

    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));
    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    // Don't emit any ticks.

    // Build a scene off the (empty) series — share the driver's series
    // handle, no deep copy needed.
    let axis =
        ContinuousAxis::new(utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 1, 1, 0, 0), 1200.0).unwrap();
    let scene = build_scene(SceneConfig {
        series: Arc::clone(&series),
        axis,
        price_range: PriceRange::new(49_900.0, 50_100.0).unwrap(),
        viewport: Viewport::new(1200.0, 400.0),
        palette: ThemePalette::dark_default(),
        calendar: crypto_spot(),
        interaction: &InteractionState::new(),
        layers: SceneLayers::all_on(),
        time_window: (utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 1, 1, 0, 0)),
        series_changed: true,
        volume_profile_config: midas_scene::VolumeProfileConfig::default(),
        volume_profile_range: 0..0,
    })
    .unwrap();

    let mut prim = ScenePrimitives::default();
    scene.paint(&mut prim);
    assert_eq!(prim.candles.len(), 0);
    // Grid still fires lines and session bands still emit a quad.
    assert!(!prim.lines.is_empty());
    assert!(!prim.quads.is_empty());

    drop(driver);
}

// ═══════════════════════════════════════════════════════════════════════
// Phase C (S10-S14) tests — XNYS, periods, holidays, EhPolicy.
// ═══════════════════════════════════════════════════════════════════════

/// Synthetic XNYS tick source — same shape as `SyntheticBtcSource` but
/// emits for a fixed AAPL symbol key. Used by the Phase C XNYS tests.
struct SyntheticXnysSource {
    tick_tx: broadcast::Sender<Arc<Tick>>,
    next_req_id: std::sync::atomic::AtomicI32,
    subscribed: Arc<AtomicBool>,
    conn_state_tx: watch::Sender<ConnectionState>,
    farm_tx: broadcast::Sender<FarmStatus>,
}

impl SyntheticXnysSource {
    fn new() -> Arc<Self> {
        let (tick_tx, _rx) = broadcast::channel::<Arc<Tick>>(4096);
        let (conn_state_tx, _) = watch::channel::<ConnectionState>(ConnectionState::Ready);
        let (farm_tx, _) = broadcast::channel::<FarmStatus>(16);
        Arc::new(Self {
            tick_tx,
            next_req_id: std::sync::atomic::AtomicI32::new(1),
            subscribed: Arc::new(AtomicBool::new(false)),
            conn_state_tx,
            farm_tx,
        })
    }

    fn spawn_emitter(
        self: &Arc<Self>,
        symbol_key: SymbolKey,
        start_ts: chrono::DateTime<Utc>,
        spacing: chrono::Duration,
        count: usize,
    ) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            while !this.subscribed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            for i in 0..count {
                let ts = start_ts + spacing * i as i32;
                let price = 180.0 + (i as f64) * 0.05;
                let tick = Tick {
                    symbol: symbol_key.clone(),
                    req_id: ReqId(1),
                    kind: TickKind::PriceSize,
                    tick_type: TickType::Last,
                    value: TickValue::PriceSize { price, size: 100 },
                    attrs: TickAttributes::default(),
                    ts,
                };
                let _ = this.tick_tx.send(Arc::new(tick));
            }
        })
    }
}

#[async_trait]
impl MarketDataSource for SyntheticXnysSource {
    async fn subscribe_ticks(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        let rx = self.tick_tx.subscribe();
        let req = ReqId(
            self.next_req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        self.subscribed.store(true, Ordering::SeqCst);
        Ok(TickStream::new(
            req,
            rx,
            Arc::new(std::sync::OnceLock::new()),
            Box::new(|| {}),
        ))
    }

    async fn subscribe_tick_by_tick(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn subscribe_realtime_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _what: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn historical_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _end: chrono::DateTime<Utc>,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn historical_stream(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    async fn resolve_contract(
        &self,
        _symbol: &SymbolKey,
        _sec_type: SecurityType,
        _exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        Err(MarketDataError::Other("not implemented".into()))
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.conn_state_tx.subscribe()
    }

    fn name(&self) -> &str {
        "synthetic-xnys"
    }
}

/// XNYS AAPL M1 end-to-end: emit ticks spanning PreMarket → Regular
/// and assert the driver's series has candles in both session kinds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xnys_aapl_m1_pipeline_pre_to_regular_sessions() {
    use midas_calendar::xnys;

    let source = SyntheticXnysSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream =
        subscribe_aggregated_bars(source_as_trait, &resolver, "AAPL", BarPeriod::m1(), clock)
            .await
            .expect("subscribe AAPL M1");

    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    assert_eq!(meta.calendar.id(), xnys().id());

    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    // 2024-01-17 (Wed) — start ticks at 09:20 ET = 14:20 UTC
    // (PreMarket) and span across 09:30 ET = 14:30 UTC (Regular).
    // PreMarket ends at 14:30 UTC, Regular opens at 14:30 UTC.
    // Spacing 1 min × 30 ticks = 30 min of wall-clock: covers 14:20 →
    // 14:50 UTC, producing PreMarket M1 bars at 14:20..14:30 and
    // Regular M1 bars at 14:30..14:50.
    let symbol_key = SymbolKey {
        contract_id: 265598, // AAPL default in StaticSymbolResolver
        symbol: "AAPL".to_string(),
    };
    let start = utc(2024, 1, 17, 14, 20, 0);
    let emitter = source.spawn_emitter(symbol_key, start, chrono::Duration::minutes(1), 30);

    // Wait for at least 15 candles — enough to ensure we cross
    // 14:30 UTC and land Regular bars in the series.
    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 15 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(wait.is_ok(), "pipeline should emit >=15 AAPL M1 bars in 5s");

    {
        let s = series.read();
        assert!(s.len() >= 15);
        // Assert we see at least one PreMarket AND at least one Regular
        // candle — the tick timeline spans 09:20–~09:50 ET.
        let mut saw_pre = false;
        let mut saw_reg = false;
        for row in s.iter() {
            match row.session_kind() {
                SessionKind::PreMarket => saw_pre = true,
                SessionKind::Regular => saw_reg = true,
                _ => {}
            }
        }
        assert!(saw_pre, "must see at least one PreMarket candle");
        assert!(saw_reg, "must see at least one Regular candle");
    }

    let _ = emitter.await;
    drop(driver);
}

/// AAPL XNYS with `EhPolicy::HideExtended` (via `Filtered<_, EhFilter>`)
/// — pre/post market bars must be filtered out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xnys_aapl_m1_eh_filter_drops_pre_market_bars() {
    use midas_calendar::xnys;
    use midas_stream::{EhFilter, Filtered};

    let source = SyntheticXnysSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream =
        subscribe_aggregated_bars(source_as_trait, &resolver, "AAPL", BarPeriod::m1(), clock)
            .await
            .expect("subscribe AAPL M1");

    // Wrap in an EH filter that drops PreMarket + PostMarket bars.
    let filtered = Filtered::new(stream, EhFilter::RTH_ONLY);

    let meta = {
        use midas_stream::BarStream;
        filtered.meta().clone()
    };
    assert_eq!(meta.calendar.id(), xnys().id());

    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    let driver = SessionChartDriver::spawn(Arc::clone(&series), filtered);

    let symbol_key = SymbolKey {
        contract_id: 265598,
        symbol: "AAPL".to_string(),
    };
    // Span 09:15 ET (PreMarket) → 10:00 ET (Regular).
    let start = utc(2024, 1, 17, 14, 15, 0);
    let emitter = source.spawn_emitter(symbol_key, start, chrono::Duration::seconds(30), 90);

    // Wait for at least a couple of Regular-session candles.
    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 2 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(
        wait.is_ok(),
        "pipeline should emit >=2 Regular-session AAPL M1 bars in 5s"
    );

    {
        let s = series.read();
        // EhFilter::RTH_ONLY drops PreMarket + PostMarket; every row
        // the driver saw must be Regular.
        for row in s.iter() {
            assert_eq!(
                row.session_kind(),
                SessionKind::Regular,
                "EH filter must drop non-Regular kinds"
            );
        }
    }

    let _ = emitter.await;
    drop(driver);
}

/// BTC crypto M1 with EH toggle — crypto's only session kind is
/// Regular, so ShowAll and HideExtended both yield the same bar
/// count (crypto has no extended hours to filter).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_eh_filter_is_noop() {
    use midas_stream::{EhFilter, Filtered};

    let source = SyntheticBtcSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream = subscribe_aggregated_bars(
        source_as_trait,
        &resolver,
        "BTC-USD",
        BarPeriod::m1(),
        clock,
    )
    .await
    .expect("subscribe BTC M1");

    let filtered = Filtered::new(stream, EhFilter::RTH_ONLY);

    let meta = {
        use midas_stream::BarStream;
        filtered.meta().clone()
    };
    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));
    let driver = SessionChartDriver::spawn(Arc::clone(&series), filtered);

    let symbol_key = SymbolKey {
        contract_id: 1_000_000_001,
        symbol: "BTC-USD".to_string(),
    };
    let start = utc(2024, 3, 1, 0, 0, 0);
    let emitter = source.spawn_emitter(symbol_key, start, chrono::Duration::seconds(9), 60);

    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 5 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(wait.is_ok());

    {
        let s = series.read();
        assert!(s.len() >= 5);
        // All rows are Regular — crypto spot only has that kind.
        for row in s.iter() {
            assert_eq!(row.session_kind(), SessionKind::Regular);
        }
    }

    let _ = emitter.await;
    drop(driver);
}

/// XNYS D1 RTH (`BarPeriod::Session(Regular)`) — feed ticks across a
/// trading day; assert the driver's series has a Partial candle
/// tagged Regular with the canonical 09:30-16:00 ET window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xnys_aapl_d1_rth_bar_window_matches_regular_session() {
    use midas_calendar::xnys;

    let source = SyntheticXnysSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream = subscribe_aggregated_bars(
        source_as_trait,
        &resolver,
        "AAPL",
        BarPeriod::d1_rth(),
        clock,
    )
    .await
    .expect("subscribe AAPL D1-RTH");

    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    assert_eq!(meta.calendar.id(), xnys().id());
    assert_eq!(meta.period, BarPeriod::d1_rth());

    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    // Emit ticks inside the 09:30 ET session (14:30 UTC is 09:30 EST).
    let symbol_key = SymbolKey {
        contract_id: 265598,
        symbol: "AAPL".to_string(),
    };
    let start = utc(2024, 1, 17, 14, 30, 0);
    let emitter = source.spawn_emitter(
        symbol_key,
        start,
        chrono::Duration::minutes(5),
        20, // 20 ticks × 5 min = 100 min of trading
    );

    // Wait for at least one candle.
    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 1 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(wait.is_ok(), "D1-RTH should emit >=1 candle in 5s");

    {
        let s = series.read();
        assert_eq!(s.len(), 1, "exactly one D1-RTH bar for one trading day");
        let row = s.at(0).expect("row 0");
        assert_eq!(row.session_kind(), SessionKind::Regular);
        // The row's ts_open should equal the session open at 14:30 UTC.
        assert_eq!(row.ts_open(), utc(2024, 1, 17, 14, 30, 0));
    }

    let _ = emitter.await;
    drop(driver);
}

/// XNYS W1 (`BarPeriod::Calendar(Week)`) — smoke test that the
/// pipeline accepts the period and emits a bar (inside a trading
/// day).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xnys_aapl_w1_calendar_week_smoke() {
    use midas_calendar::xnys;

    let source = SyntheticXnysSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream =
        subscribe_aggregated_bars(source_as_trait, &resolver, "AAPL", BarPeriod::w1(), clock)
            .await
            .expect("subscribe AAPL W1");

    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    assert_eq!(meta.calendar.id(), xnys().id());
    assert_eq!(meta.period, BarPeriod::w1());

    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    // Emit ticks inside the 09:30 ET Regular session on a Wednesday.
    let symbol_key = SymbolKey {
        contract_id: 265598,
        symbol: "AAPL".to_string(),
    };
    let start = utc(2024, 1, 17, 14, 35, 0);
    let emitter = source.spawn_emitter(symbol_key, start, chrono::Duration::minutes(10), 10);

    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 1 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(wait.is_ok(), "W1 should emit >=1 candle in 5s");

    {
        let s = series.read();
        assert_eq!(s.len(), 1);
        let row = s.at(0).expect("row 0");
        assert_eq!(row.session_kind(), SessionKind::Regular);
    }

    let _ = emitter.await;
    drop(driver);
}

/// BTC crypto D1 (via `BarPeriod::Session(Regular)` mapped to 24h UTC)
/// — assert the series accepts the period and the bar window runs
/// 00:00 UTC → 00:00 UTC.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_btc_d1_regular_is_24h_utc() {
    let source = SyntheticBtcSource::new();
    let source_as_trait: Arc<dyn MarketDataSource> =
        Arc::clone(&source) as Arc<dyn MarketDataSource>;

    let resolver = StaticSymbolResolver::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let stream = subscribe_aggregated_bars(
        source_as_trait,
        &resolver,
        "BTC-USD",
        BarPeriod::d1_rth(),
        clock,
    )
    .await
    .expect("subscribe BTC D1-regular");

    let meta = {
        use midas_stream::BarStream;
        stream.meta().clone()
    };
    assert_eq!(meta.period, BarPeriod::d1_rth());
    assert_eq!(meta.calendar.id(), crypto_spot().id());

    let series = Arc::new(parking_lot::RwLock::new(CandleSeries::new(
        meta.calendar.id(),
        meta.period,
        meta.symbol,
    )));

    let driver = SessionChartDriver::spawn(Arc::clone(&series), stream);

    let symbol_key = SymbolKey {
        contract_id: 1_000_000_001,
        symbol: "BTC-USD".to_string(),
    };
    let start = utc(2024, 3, 1, 12, 0, 0); // mid-day UTC
    let emitter = source.spawn_emitter(symbol_key, start, chrono::Duration::minutes(5), 10);

    let mut rx = driver.version_receiver();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let len = { series.read().len() };
            if len >= 1 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), rx.changed()).await;
        }
    })
    .await;
    assert!(wait.is_ok(), "crypto D1 regular should emit >=1 bar");

    {
        let s = series.read();
        let row = s.at(0).expect("row 0");
        // Crypto's D1 "Regular" window is 00:00 UTC of the day.
        assert_eq!(row.ts_open(), utc(2024, 3, 1, 0, 0, 0));
        assert_eq!(row.session_kind(), SessionKind::Regular);
    }

    let _ = emitter.await;
    drop(driver);
}

/// Phase C widget tests — calendar-agnostic axis kind, EH policy
/// cycling, scene-builder integration with XNYS.
mod widget_integration {
    use super::*;
    use midas_app::session_chart::{AxisKind, EhPolicy, SessionChart};
    use midas_axis::{for_calendar, PriceRange, Viewport};
    use midas_calendar::{crypto_spot, xnys};
    use midas_scene::ThemePalette;

    fn empty_series_for(
        calendar: &'static dyn midas_calendar::ExchangeCalendar,
        ticker: &str,
    ) -> Arc<parking_lot::RwLock<CandleSeries>> {
        let sym = Symbol::from_ticker_leak(ticker, calendar.id());
        Arc::new(parking_lot::RwLock::new(CandleSeries::new(
            calendar.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    fn dummy_driver(
        calendar: &'static dyn midas_calendar::ExchangeCalendar,
        ticker: &str,
    ) -> Arc<SessionChartDriver> {
        use async_trait::async_trait;
        use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
        use tokio::sync::mpsc;

        struct EmptyStream {
            meta: BarStreamMeta,
            _rx: mpsc::Receiver<()>,
        }
        #[async_trait]
        impl BarStream for EmptyStream {
            fn meta(&self) -> &BarStreamMeta {
                &self.meta
            }
            async fn next(&mut self) -> Option<midas_bars::Candle> {
                // Park forever on the receiver.
                self._rx.recv().await.map(|_| unreachable!())
            }
            async fn snapshot(
                &mut self,
                _range: TimeRange,
            ) -> Result<Vec<midas_bars::Candle>, StreamError> {
                Err(StreamError::NotSeekable)
            }
        }

        let series = empty_series_for(calendar, ticker);
        let sym = Symbol::from_ticker_leak(ticker, calendar.id());
        let (_tx, rx) = mpsc::channel(1);
        let meta = BarStreamMeta::new(sym, calendar, BarPeriod::m1());
        let stream = EmptyStream { meta, _rx: rx };
        Arc::new(SessionChartDriver::spawn(series, stream))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_widget_axis_kind_is_continuous() {
        let driver = dummy_driver(crypto_spot(), "BTC-USD");
        let w = SessionChart::new(
            driver,
            crypto_spot(),
            BarPeriod::m1(),
            PriceRange::new(1.0, 100_000.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            ThemePalette::dark_default(),
            (utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 2, 0, 0, 0)),
        )
        .expect("canonical widget inputs");
        assert_eq!(w.axis_kind(), AxisKind::Continuous);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_widget_axis_kind_is_compressed() {
        let driver = dummy_driver(xnys(), "AAPL");
        let w = SessionChart::new(
            driver,
            xnys(),
            BarPeriod::m1(),
            PriceRange::new(1.0, 1000.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            ThemePalette::dark_default(),
            (utc(2024, 1, 17, 0, 0, 0), utc(2024, 1, 19, 0, 0, 0)),
        )
        .expect("canonical widget inputs");
        assert_eq!(w.axis_kind(), AxisKind::Compressed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eh_policy_cycles_three_times_back_to_show_all() {
        let driver = dummy_driver(xnys(), "AAPL");
        let mut w = SessionChart::new(
            driver,
            xnys(),
            BarPeriod::m1(),
            PriceRange::new(1.0, 1000.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            ThemePalette::dark_default(),
            (utc(2024, 1, 17, 0, 0, 0), utc(2024, 1, 19, 0, 0, 0)),
        )
        .expect("canonical widget inputs");
        assert_eq!(w.eh_policy(), EhPolicy::ShowAll);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::HideExtended);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowBarsOnly);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowAll);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_build_scene_with_for_calendar_axis_produces_compressed_axis() {
        use midas_calendar::TimeAxisPolicy;
        // Confirms `midas_axis::for_calendar` correctly returns a
        // compressed axis on XNYS and a continuous axis on crypto.
        let axis = for_calendar(
            xnys(),
            (utc(2024, 1, 17, 0, 0, 0), utc(2024, 1, 19, 0, 0, 0)),
            1000.0,
        );
        assert_eq!(axis.policy(), TimeAxisPolicy::CompressedSessionBoundaries);
        let axis2 = for_calendar(
            crypto_spot(),
            (utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 2, 0, 0, 0)),
            1000.0,
        );
        assert_eq!(axis2.policy(), TimeAxisPolicy::Continuous);
    }
}

/// Phase D (shader-widget integration) tests — CPU-side assertions on
/// the `SessionChart::paint_buckets()` pipeline. These do NOT require
/// a GPU; they exercise the full `widget → paint → translate` chain
/// and verify bucket shape on both crypto (no session bands) and
/// XNYS (session-band quads emitted).
///
/// See `desktop/win/crates/midas-app/src/session_chart/shader.rs`
/// for the GPU-side wiring that consumes these buckets.
mod paint_buckets_phase_d {
    use super::*;
    use midas_app::session_chart::{SessionChart, SessionChartDriver};
    use midas_axis::{PriceRange, Viewport};
    use midas_bars::Symbol;
    use midas_calendar::{crypto_spot, xnys};
    use midas_scene::ThemePalette;

    /// Dummy driver that never receives candles. The widget still
    /// paints a grid + axis; the test just needs a live driver Arc.
    fn dummy_driver(
        calendar: &'static dyn midas_calendar::ExchangeCalendar,
        ticker: &str,
    ) -> (
        std::sync::Arc<SessionChartDriver>,
        std::sync::Arc<parking_lot::RwLock<CandleSeries>>,
    ) {
        use async_trait::async_trait;
        use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
        use tokio::sync::mpsc;

        struct EmptyStream {
            meta: BarStreamMeta,
            _rx: mpsc::Receiver<()>,
        }
        #[async_trait]
        impl BarStream for EmptyStream {
            fn meta(&self) -> &BarStreamMeta {
                &self.meta
            }
            async fn next(&mut self) -> Option<midas_bars::Candle> {
                self._rx.recv().await.map(|_| unreachable!())
            }
            async fn snapshot(
                &mut self,
                _range: TimeRange,
            ) -> Result<Vec<midas_bars::Candle>, StreamError> {
                Err(StreamError::NotSeekable)
            }
        }

        let sym = Symbol::from_ticker_leak(ticker, calendar.id());
        let series = std::sync::Arc::new(parking_lot::RwLock::new(CandleSeries::new(
            calendar.id(),
            BarPeriod::m1(),
            sym,
        )));
        let (_tx, rx) = mpsc::channel(1);
        let meta = BarStreamMeta::new(sym, calendar, BarPeriod::m1());
        let stream = EmptyStream { meta, _rx: rx };
        (
            std::sync::Arc::new(SessionChartDriver::spawn(
                std::sync::Arc::clone(&series),
                stream,
            )),
            series,
        )
    }

    /// Feed 5 synthetic crypto candles directly into the shared series
    /// (bypassing the pump task so the test is deterministic), build a
    /// SessionChart, call `paint_buckets()`, assert the candles +
    /// grid buckets are non-empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_widget_paint_buckets_has_candles_and_grid() {
        use chrono::TimeZone;
        use midas_bars::{Candle, Completeness, Ohlcv};

        let cal = crypto_spot();
        let (driver, series) = dummy_driver(cal, "BTC-USD");

        // Push 5 candles directly.
        {
            let mut g = series.write();
            let sym = Symbol::new("BTC-USD", cal.id());
            let start = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
            for i in 0..5i64 {
                let ts = start + chrono::Duration::minutes(i);
                let session = cal.classify(ts);
                let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
                let price = 50_000.0 + (i as f64) * 5.0;
                let ohlcv =
                    Ohlcv::new(price, price + 10.0, price - 10.0, price + 5.0, 1, 1, None).unwrap();
                let candle = Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap();
                g.push(candle);
            }
        }

        let start = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let end = start + chrono::Duration::hours(1);
        let mut widget = SessionChart::new(
            std::sync::Arc::clone(&driver),
            cal,
            BarPeriod::m1(),
            PriceRange::new(49_900.0, 50_100.0).unwrap(),
            Viewport::new(1200.0, 400.0),
            ThemePalette::dark_default(),
            (start, end),
        )
        .expect("canonical widget inputs");

        let buckets = widget.paint_buckets();
        assert_eq!(
            buckets.candles.len(),
            5,
            "candles bucket should reflect the 5 pushed candles"
        );
        assert!(
            !buckets.lines.is_empty(),
            "grid + axis layer should emit lines"
        );
        // Crypto has no session bands — but the VolumeLayer does emit
        // a quad for the bottom-strip volume area, so `quads` is
        // non-empty for a different reason. Assert only on the shape
        // of the bucket container, not on zero-length quads.
        drop(driver);
    }

    /// XNYS: session bands produce `quads` because `CompressedAxis`
    /// + `SessionBandLayer` emits tinted rectangles for PreMarket /
    /// Regular / PostMarket. The grid + candle axis still renders
    /// lines.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_widget_paint_buckets_emits_session_band_quads() {
        let cal = xnys();
        let (driver, _series) = dummy_driver(cal, "AAPL");

        // Two weekdays for the compressed axis to have sessions.
        let start = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 0, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2024, 1, 19, 0, 0, 0).unwrap();
        let mut widget = SessionChart::new(
            std::sync::Arc::clone(&driver),
            cal,
            BarPeriod::m1(),
            PriceRange::new(180.0, 200.0).unwrap(),
            Viewport::new(1200.0, 400.0),
            ThemePalette::dark_default(),
            (start, end),
        )
        .expect("canonical xnys widget inputs");

        let buckets = widget.paint_buckets();
        // No candles pushed — candle bucket is empty by design.
        assert_eq!(buckets.candles.len(), 0);
        // Session bands paint every visible XNYS session kind as a
        // tinted quad; with two weekdays in the window we expect
        // multiple quads.
        assert!(
            !buckets.quads.is_empty(),
            "XNYS session-band layer must emit at least one quad"
        );
        // Grid + session-separator layers emit thin lines too.
        assert!(!buckets.lines.is_empty());

        drop(driver);
    }

    /// Empty series → paint_buckets still yields a paintable scene.
    /// This is the "first-frame-after-window-open" smoke: the GPU
    /// path must not crash when the driver hasn't produced any
    /// candles yet.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_series_paint_buckets_is_valid() {
        let cal = crypto_spot();
        let (driver, _series) = dummy_driver(cal, "BTC-USD");
        let start = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let end = start + chrono::Duration::hours(1);
        let mut widget = SessionChart::new(
            std::sync::Arc::clone(&driver),
            cal,
            BarPeriod::m1(),
            PriceRange::new(49_900.0, 50_100.0).unwrap(),
            Viewport::new(1200.0, 400.0),
            ThemePalette::dark_default(),
            (start, end),
        )
        .expect("canonical widget inputs");

        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 0);
        // Grid always emits.
        assert!(!b.lines.is_empty());
        drop(driver);
    }
}
