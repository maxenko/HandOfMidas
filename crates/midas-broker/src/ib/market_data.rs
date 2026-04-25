//! [`IbMarketData`] — router-era IB adapter implementing
//! [`MarketDataSource`](crate::MarketDataSource).
//!
//! Successor to the retired standalone `IbClient` / `IbDataSource`
//! adapters. The path differs in three ways:
//!
//! 1. Object-safe `#[async_trait]` trait surface (M-1).
//! 2. Per-subscription publisher tasks owning the rust-ibapi
//!    `Subscription` directly; cancel-on-drop of our handle aborts the
//!    publisher, which drops the subscription, which auto-cancels via
//!    rust-ibapi's own `Drop` — single cancel path (no double-send).
//! 3. [`PacingGovernor`](super::pacing::PacingGovernor) guards every
//!    streaming / historical call (BR-19).

use std::sync::atomic::AtomicI32;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "test_inject"))]
use midas_broker_core::market_data::MarketEvent;
use midas_broker_core::market_data::{
    Bar, ConnectionState, ContractDetails, FarmCode, FarmStatus, GenericTicks, IbDuration,
    MarketDataError, ReqId, SecurityType, SymbolKey, TickByTickKind, Timeframe, WhatToShow,
};
use midas_broker_core::market_data::{Tick, TickAttributes, TickKind, TickType, TickValue};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::ib::config::IbMarketDataConfig;
use crate::ib::pacing::{IdenticalKey, PacingGovernor, StreamingLineGuard};
use crate::ib::translation as tr;
use crate::market_data_source::{HistoricalBarsResult, MarketDataSource};
use crate::stream::{HistoricalStream, HistoricalStreamEvent, RealtimeBarStream, TickStream};

/// Wrap a rust-ibapi await with [`IbMarketDataConfig::ib_op_timeout`].
///
/// The async branch returns whatever the inner future returns; on
/// `Elapsed` we synthesise a `MarketDataError::Other` carrying the
/// call label so logs can distinguish which provider call blew the
/// budget.
async fn with_ib_timeout<F, T>(
    timeout: Duration,
    label: &'static str,
    fut: F,
) -> Result<T, MarketDataError>
where
    F: std::future::Future<Output = Result<T, MarketDataError>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(MarketDataError::Other(format!(
            "ib {label} timed out after {timeout:?}"
        ))),
    }
}

/// `with_ib_timeout` label used by both the production `connect` path
/// and tests that exercise [`wait_for_mkt_farm_up`]. Keeping the literal
/// in one place keeps test assertions and operator log fields in sync
/// when the label is renamed.
const FARM_UP_LABEL: &str = "farm_up_mkt";

/// Drain `farm_rx` until the first MKT data-farm-up event.
///
/// Implements the M-23 gate used by [`IbMarketData::connect`]:
///
/// * Returns `Ok(())` on the first
///   `FarmStatus { code: MarketDataFarmOk, connected: true, .. }`.
/// * Ignores other farm transitions (e.g. HMDS/SecDef `Ok`, MKT
///   `Inactive` / `Broken`) — callers that care about those subscribe
///   to [`MarketDataSource::farm_status`] directly.
/// * Treats [`broadcast::error::RecvError::Lagged`] as non-fatal (a
///   slow consumer is not a disconnect) and continues looping.
/// * Returns `Err(..)` on [`broadcast::error::RecvError::Closed`] — the
///   sender was dropped, so no more events can arrive.
async fn wait_for_mkt_farm_up(
    farm_rx: &mut broadcast::Receiver<FarmStatus>,
) -> Result<(), MarketDataError> {
    loop {
        match farm_rx.recv().await {
            Ok(s) if s.code == FarmCode::MarketDataFarmOk && s.connected => return Ok(()),
            Ok(_) => continue, // ignore other farm transitions
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => {
                return Err(MarketDataError::Other(
                    "farm-status channel closed before MKT farm-up".into(),
                ));
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// IbMarketData
// ───────────────────────────────────────────────────────────────────────────

/// Router-era IB market-data adapter.
///
/// Internally holds an `Arc<ibapi::Client>` (set on connect). Every
/// subscription method spawns a publisher task that drains the
/// rust-ibapi `Subscription<T>` and pushes translated events onto a
/// broadcast channel; the consumer handle (our
/// [`TickStream`]/[`RealtimeBarStream`]/[`HistoricalStream`]) owns the
/// receiver plus a cancel closure that aborts the publisher task.
///
/// Connection handshake and the nextValidId / farm-status bootstrap
/// sequence (M-14 / M-23) run from [`Self::connect`].
pub struct IbMarketData {
    config: IbMarketDataConfig,
    client: tokio::sync::RwLock<Option<Arc<ibapi::Client>>>,
    pacing: Arc<PacingGovernor>,
    req_id_counter: AtomicI32,
    farm_status_tx: broadcast::Sender<FarmStatus>,
    conn_state_tx: watch::Sender<ConnectionState>,
    ordering_ready_tx: watch::Sender<Option<i32>>,
}

impl IbMarketData {
    /// Build a new adapter with the supplied config. No connection
    /// attempt is made — call [`Self::connect`] to bring it up.
    pub fn new(config: IbMarketDataConfig) -> Self {
        let (farm_status_tx, _) = broadcast::channel(256);
        let (conn_state_tx, _) = watch::channel(ConnectionState::Disconnected);
        let (ordering_ready_tx, _) = watch::channel(None::<i32>);
        let pacing = Arc::new(PacingGovernor::new(config.pacing));
        Self {
            config,
            client: tokio::sync::RwLock::new(None),
            pacing,
            req_id_counter: AtomicI32::new(1),
            farm_status_tx,
            conn_state_tx,
            ordering_ready_tx,
        }
    }

    /// Connect to TWS / IB Gateway using the configured address and
    /// client id.
    ///
    /// Updates `connection_state` to `Connecting` → `Connected` on the
    /// way up. The router's retry logic (S5) owns the `Reconnecting`
    /// transition — this method is single-shot.
    pub async fn connect(&self) -> Result<(), MarketDataError> {
        // Defence-in-depth live-trading guard: `BrokerConfig::validate`
        // catches this at TOML load, but programmatic construction /
        // post-construction mutation (e.g. `cfg.port = 4001`) never
        // flows through `validate()`. Fail fast before any I/O.
        if self.config.port == 4001 && !self.config.allow_live {
            return Err(MarketDataError::LiveTradingNotConfirmed);
        }
        let _ = self.conn_state_tx.send(ConnectionState::Connecting);
        let address = self.config.address();
        let ib_timeout = self.config.ib_op_timeout;
        let client = with_ib_timeout(ib_timeout, "connect", async {
            ibapi::Client::connect(&address, self.config.client_id)
                .await
                .map_err(|e| MarketDataError::Other(format!("ib connect: {e}")))
        })
        .await?;
        let server_version = client.server_version();
        {
            let mut w = self.client.write().await;
            *w = Some(Arc::new(client));
        }
        let _ = self
            .conn_state_tx
            .send(ConnectionState::Connected { server_version });
        // M-14/M-23: fetch nextValidId once connected so OrderClient
        // callers block on `ordering_ready_tx` getting `Some(_)`.
        //
        // Subscribe to `farm_status_tx` BEFORE awaiting
        // `next_valid_order_id` — otherwise a fast gateway that emits
        // `MarketDataFarmOk` between `nextValidId` returning and the
        // farm-up loop arming would deadlock the gate until timeout.
        // The broadcast receiver buffers everything from subscription
        // onward, so subscribing early and then draining later is safe.
        let mut farm_rx = self.farm_status_tx.subscribe();
        if let Some(c) = self.client.read().await.clone() {
            let id_fut = async {
                c.next_valid_order_id()
                    .await
                    .map_err(|e| MarketDataError::Other(format!("next_valid_order_id: {e}")))
            };
            if let Ok(id) = with_ib_timeout(ib_timeout, "next_valid_order_id", id_fut).await {
                let _ = self.ordering_ready_tx.send(Some(id));

                // M-23: gate `Ready` on the first MKT data-farm-up
                // event. If farm-up never arrives within `ib_timeout`,
                // the connect call returns the timeout error and we
                // stay in `Connected { .. }` — caller observes a
                // failed connect, not a stuck `Connecting`.
                with_ib_timeout(
                    ib_timeout,
                    FARM_UP_LABEL,
                    wait_for_mkt_farm_up(&mut farm_rx),
                )
                .await?;
                let _ = self.conn_state_tx.send(ConnectionState::Ready);
            }
        }
        Ok(())
    }

    /// Cheap clone of the connected rust-ibapi client, or
    /// `Err(Disconnected)` if none is set.
    async fn client(&self) -> Result<Arc<ibapi::Client>, MarketDataError> {
        self.client
            .read()
            .await
            .clone()
            .ok_or(MarketDataError::Disconnected)
    }

    /// Watch receiver for `OrderingReady { next_order_id }` (M-14).
    ///
    /// Used by [`IbOrderClient`](super::order_client::IbOrderClient) to
    /// block `next_order_id()` until the adapter has observed
    /// `nextValidId`.
    pub fn ordering_ready(&self) -> watch::Receiver<Option<i32>> {
        self.ordering_ready_tx.subscribe()
    }

    /// Shared pacing governor handle. Exposed for the order-client
    /// sibling in this module.
    #[allow(dead_code)] // S5 router consumer lands in the next slice
    pub(crate) fn pacing(&self) -> Arc<PacingGovernor> {
        Arc::clone(&self.pacing)
    }

    /// Cheap clone of the rust-ibapi client or `None` if not connected.
    ///
    /// Exposed crate-wide so the [`IbOrderClient`](super::order_client::IbOrderClient)
    /// sibling can share the same underlying TCP session.
    pub(crate) async fn client_handle(&self) -> Option<Arc<ibapi::Client>> {
        self.client.read().await.clone()
    }

    /// Per-operation upstream deadline taken from
    /// [`IbMarketDataConfig::ib_op_timeout`]. Exposed crate-wide so
    /// [`super::order_client::IbOrderClient`] can wrap its own
    /// rust-ibapi awaits with the same budget.
    pub(crate) fn ib_op_timeout(&self) -> Duration {
        self.config.ib_op_timeout
    }

    /// Per-leg deadline for bracket cancels, taken from
    /// [`IbMarketDataConfig::cancel_leg_timeout`]. Exposed crate-wide
    /// so the bracket-submission helper can apply it uniformly.
    ///
    /// Lives on the market adapter rather than the order client so
    /// both the router and the bracket submitter can reach the same
    /// knob through whichever handle they already carry. Currently
    /// the desktop-side `bracket_submit::cancel_bracket` uses its
    /// own module-local constant — this accessor is plumbed so a
    /// future slice can plumb the config knob all the way through.
    #[allow(dead_code)]
    pub(crate) fn cancel_leg_timeout(&self) -> Duration {
        self.config.cancel_leg_timeout
    }

    fn next_req_id(&self) -> ReqId {
        ReqId::next(&self.req_id_counter)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// MarketDataSource impl
// ───────────────────────────────────────────────────────────────────────────

#[async_trait]
impl MarketDataSource for IbMarketData {
    async fn subscribe_ticks(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        let client = self.client().await?;
        let guard = self.pacing.acquire_streaming_line()?;
        let req_id = self.next_req_id();
        let symbol_owned = symbol.clone();
        let _ = con_id;
        let (tx, rx) = broadcast::channel::<Arc<Tick>>(4096);
        let last_error: Arc<OnceLock<MarketDataError>> = Arc::new(OnceLock::new());
        let contract = tr::build_ib_stock_contract(symbol, &self.config.default_exchange);
        let tick_strings: Vec<String> = tr::generic_ticks_as_vec(&generic_ticks);
        let tick_refs: Vec<&str> = tick_strings.iter().map(|s| s.as_str()).collect();
        let ib_sub = with_ib_timeout(self.config.ib_op_timeout, "subscribe_ticks", async {
            client
                .market_data(&contract)
                .generic_ticks(&tick_refs)
                .subscribe()
                .await
                .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
        })
        .await?;
        let task = spawn_tick_publisher(
            ib_sub,
            symbol_owned,
            req_id,
            tx,
            Arc::clone(&last_error),
            guard,
        );
        Ok(TickStream::new(
            req_id,
            rx,
            last_error,
            Box::new(move || task.abort()),
        ))
    }

    async fn subscribe_tick_by_tick(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        let client = self.client().await?;
        let guard = self.pacing.acquire_streaming_line()?;
        let req_id = self.next_req_id();
        let symbol_owned = symbol.clone();
        let _ = con_id;
        let (tx, rx) = broadcast::channel::<Arc<Tick>>(4096);
        let last_error: Arc<OnceLock<MarketDataError>> = Arc::new(OnceLock::new());
        let contract = tr::build_ib_stock_contract(symbol, &self.config.default_exchange);
        let ib_timeout = self.config.ib_op_timeout;
        let task: JoinHandle<()> = match kind {
            TickByTickKind::Last => {
                let sub = with_ib_timeout(ib_timeout, "tick_by_tick_last", async {
                    client
                        .tick_by_tick_last(&contract, 0, false)
                        .await
                        .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
                })
                .await?;
                spawn_tbt_trade_publisher(
                    sub,
                    symbol_owned,
                    req_id,
                    tx,
                    Arc::clone(&last_error),
                    guard,
                    false,
                )
            }
            TickByTickKind::AllLast => {
                let sub = with_ib_timeout(ib_timeout, "tick_by_tick_all_last", async {
                    client
                        .tick_by_tick_all_last(&contract, 0, false)
                        .await
                        .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
                })
                .await?;
                spawn_tbt_trade_publisher(
                    sub,
                    symbol_owned,
                    req_id,
                    tx,
                    Arc::clone(&last_error),
                    guard,
                    true,
                )
            }
            TickByTickKind::BidAsk => {
                let sub = with_ib_timeout(ib_timeout, "tick_by_tick_bid_ask", async {
                    client
                        .tick_by_tick_bid_ask(&contract, 0, false)
                        .await
                        .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
                })
                .await?;
                spawn_tbt_bidask_publisher(
                    sub,
                    symbol_owned,
                    req_id,
                    tx,
                    Arc::clone(&last_error),
                    guard,
                )
            }
            TickByTickKind::MidPoint => {
                let sub = with_ib_timeout(ib_timeout, "tick_by_tick_midpoint", async {
                    client
                        .tick_by_tick_midpoint(&contract, 0, false)
                        .await
                        .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
                })
                .await?;
                spawn_tbt_midpoint_publisher(
                    sub,
                    symbol_owned,
                    req_id,
                    tx,
                    Arc::clone(&last_error),
                    guard,
                )
            }
        };
        Ok(TickStream::new(
            req_id,
            rx,
            last_error,
            Box::new(move || task.abort()),
        ))
    }

    async fn subscribe_realtime_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        let client = self.client().await?;
        let guard = self.pacing.acquire_streaming_line()?;
        let req_id = self.next_req_id();
        let symbol_owned = symbol.clone();
        let _ = con_id;
        let (tx, rx) = broadcast::channel::<Arc<Bar>>(2048);
        let last_error: Arc<OnceLock<MarketDataError>> = Arc::new(OnceLock::new());
        let contract = tr::build_ib_stock_contract(symbol, &self.config.default_exchange);
        let ib_sub = with_ib_timeout(
            self.config.ib_op_timeout,
            "subscribe_realtime_bars",
            async {
                client
                    .realtime_bars(
                        &contract,
                        tr::to_ib_realtime_bar_size(Timeframe::S5),
                        tr::to_ib_realtime_what_to_show(what_to_show),
                        ibapi::market_data::TradingHours::Regular,
                    )
                    .await
                    .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
            },
        )
        .await?;
        let task = spawn_rt_bar_publisher(
            ib_sub,
            symbol_owned,
            what_to_show,
            tx,
            Arc::clone(&last_error),
            guard,
        );
        Ok(RealtimeBarStream::new(
            req_id,
            rx,
            last_error,
            Box::new(move || task.abort()),
        ))
    }

    async fn historical_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        end: DateTime<Utc>,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        let client = self.client().await?;
        let key = IdenticalKey::new(con_id, bar_size, what_to_show, use_rth);
        self.pacing.acquire_historical(key).await?;
        let contract = tr::build_ib_stock_contract(symbol, &self.config.default_exchange);
        let end_ib = tr::chrono_to_offsetdatetime(end);
        let data = with_ib_timeout(self.config.ib_op_timeout, "historical_data", async {
            client
                .historical_data(
                    &contract,
                    Some(end_ib),
                    tr::to_ib_historical_duration(duration),
                    tr::to_ib_historical_bar_size(bar_size),
                    Some(tr::to_ib_historical_what_to_show(what_to_show)),
                    if use_rth {
                        ibapi::market_data::TradingHours::Regular
                    } else {
                        ibapi::market_data::TradingHours::Extended
                    },
                )
                .await
                .map_err(|e| ib_error_to_market_data_error(&e, &symbol.symbol))
        })
        .await?;
        let bars = tr::translate_historical_payload(symbol, bar_size, &data);
        if bars.is_empty() {
            return Err(MarketDataError::Other(
                "IB returned empty historical payload".into(),
            ));
        }
        let first_ts = bars.first().map(|b| b.ts_open).unwrap_or_else(Utc::now);
        let last_ts = bars.last().map(|b| b.ts_close).unwrap_or_else(Utc::now);
        Ok(HistoricalBarsResult {
            bars,
            first_ts,
            last_ts,
        })
    }

    async fn historical_stream(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        let client = self.client().await?;
        let key = IdenticalKey::new(con_id, bar_size, what_to_show, use_rth);
        self.pacing.acquire_historical(key).await?;
        let req_id = self.next_req_id();
        let symbol_owned = symbol.clone();
        let contract = tr::build_ib_stock_contract(symbol, &self.config.default_exchange);
        let ib_sub = with_ib_timeout(
            self.config.ib_op_timeout,
            "historical_data_streaming",
            async {
                client
                    .historical_data_streaming(
                        &contract,
                        tr::to_ib_historical_duration(duration),
                        tr::to_ib_historical_bar_size(bar_size),
                        Some(tr::to_ib_historical_what_to_show(what_to_show)),
                        if use_rth {
                            ibapi::market_data::TradingHours::Regular
                        } else {
                            ibapi::market_data::TradingHours::Extended
                        },
                        true, // keep_up_to_date: emit live updates after initial batch
                    )
                    .await
                    .map_err(|e| ib_error_to_market_data_error(&e, &symbol_owned.symbol))
            },
        )
        .await?;
        let (tx, rx) = mpsc::channel::<HistoricalStreamEvent>(256);
        let task = spawn_historical_publisher(ib_sub, symbol_owned, bar_size, tx);
        Ok(HistoricalStream::new(
            req_id,
            rx,
            Box::new(move || task.abort()),
        ))
    }

    async fn resolve_contract(
        &self,
        symbol: &SymbolKey,
        sec_type: SecurityType,
        exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        let client = self.client().await?;
        let contract = ibapi::contracts::Contract {
            contract_id: symbol.contract_id,
            symbol: symbol.symbol.clone().into(),
            security_type: tr::to_ib_security_type(sec_type),
            exchange: exchange.into(),
            currency: "USD".into(),
            ..ibapi::contracts::Contract::default()
        };
        let details = with_ib_timeout(self.config.ib_op_timeout, "contract_details", async {
            client
                .contract_details(&contract)
                .await
                .map_err(|e| ib_error_to_market_data_error(&e, &symbol.symbol))
        })
        .await?;
        details
            .first()
            .map(tr::translate_contract_details)
            .ok_or_else(|| MarketDataError::Other(format!("no contract details for {symbol}")))
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_status_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.conn_state_tx.subscribe()
    }

    fn name(&self) -> &str {
        "ib"
    }

    /// **OPEN**: the real IB source leaves `inject_for_test` as its
    /// default no-op body. Slice 7 may wire an explicit error path so
    /// harness-driven tests can simulate farm-down / permission-denied
    /// flows without an actual TWS connection.
    #[cfg(any(test, feature = "test_inject"))]
    fn inject_for_test(&self, _event: MarketEvent) {}
}

// ───────────────────────────────────────────────────────────────────────────
// Publisher tasks
//
// Each spawns a publisher task that OWNS the rust-ibapi subscription.
// Our stream handle's Drop aborts the task (single cancel path — the
// task drops the rust-ibapi sub, which auto-cancels upstream).
// ───────────────────────────────────────────────────────────────────────────

/// Shared helper: set `last_error` the first time and return.
fn latch_error(last_error: &Arc<OnceLock<MarketDataError>>, err: MarketDataError) {
    let _ = last_error.set(err);
}

fn ib_error_to_market_data_error(e: &ibapi::Error, symbol: &str) -> MarketDataError {
    match e {
        ibapi::Error::Message(code, msg) => match *code {
            10089 => MarketDataError::NoPermission {
                symbol: symbol.to_string(),
            },
            10167 => MarketDataError::RequiresAdditionalSubscription {
                symbol: symbol.to_string(),
            },
            100..=102 => MarketDataError::PacingViolation(msg.clone()),
            _ => MarketDataError::Other(format!("ib error [{code}] {msg}")),
        },
        ibapi::Error::ConnectionFailed | ibapi::Error::ConnectionReset => {
            MarketDataError::Disconnected
        }
        other => MarketDataError::Other(format!("ib error: {other}")),
    }
}

fn spawn_tick_publisher(
    mut ib_sub: ibapi::subscriptions::Subscription<ibapi::market_data::realtime::TickTypes>,
    symbol: SymbolKey,
    req_id: ReqId,
    tx: broadcast::Sender<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    _line_guard: StreamingLineGuard,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let next = ib_sub.next().await;
            match next {
                Some(Ok(ev)) => {
                    if let Some(tick) = tr::translate_tick_event(&symbol, req_id, Utc::now(), ev) {
                        if tx.send(Arc::new(tick)).is_err() {
                            // All receivers gone — the router no longer
                            // cares; let the drop closure cancel.
                            break;
                        }
                    }
                }
                Some(Err(e)) => {
                    latch_error(
                        &last_error,
                        ib_error_to_market_data_error(&e, &symbol.symbol),
                    );
                    break;
                }
                None => break,
            }
        }
        // Publisher exit; upstream rust-ibapi Subscription Drop cancels
        // via its own closure. `_line_guard` drops here, freeing the
        // streaming-line slot.
        let _ = ib_sub;
    })
}

fn spawn_tbt_trade_publisher(
    mut ib_sub: ibapi::subscriptions::Subscription<ibapi::market_data::realtime::Trade>,
    symbol: SymbolKey,
    req_id: ReqId,
    tx: broadcast::Sender<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    _line_guard: StreamingLineGuard,
    _all_last: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match ib_sub.next().await {
                Some(Ok(trade)) => {
                    let ts = tr::offsetdatetime_to_chrono(trade.time);
                    let tick = Tick {
                        symbol: symbol.clone(),
                        req_id,
                        kind: TickKind::PriceSize,
                        tick_type: TickType::Last,
                        value: TickValue::PriceSize {
                            price: trade.price,
                            size: trade.size as i64,
                        },
                        attrs: TickAttributes {
                            past_limit: trade.trade_attribute.past_limit,
                            unreported: trade.trade_attribute.unreported,
                            ..TickAttributes::default()
                        },
                        ts,
                    };
                    if tx.send(Arc::new(tick)).is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    latch_error(
                        &last_error,
                        ib_error_to_market_data_error(&e, &symbol.symbol),
                    );
                    break;
                }
                None => break,
            }
        }
        let _ = ib_sub;
    })
}

fn spawn_tbt_bidask_publisher(
    mut ib_sub: ibapi::subscriptions::Subscription<ibapi::market_data::realtime::BidAsk>,
    symbol: SymbolKey,
    req_id: ReqId,
    tx: broadcast::Sender<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    _line_guard: StreamingLineGuard,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match ib_sub.next().await {
                Some(Ok(ba)) => {
                    let ts = tr::offsetdatetime_to_chrono(ba.time);
                    // Emit two atomic price-size ticks — one Bid, one Ask.
                    // M-17: keep each as a single event (atomic pair).
                    let bid_tick = Tick {
                        symbol: symbol.clone(),
                        req_id,
                        kind: TickKind::PriceSize,
                        tick_type: TickType::Bid,
                        value: TickValue::PriceSize {
                            price: ba.bid_price,
                            size: ba.bid_size as i64,
                        },
                        attrs: TickAttributes {
                            bid_past_low: ba.bid_ask_attribute.bid_past_low,
                            ..TickAttributes::default()
                        },
                        ts,
                    };
                    let ask_tick = Tick {
                        symbol: symbol.clone(),
                        req_id,
                        kind: TickKind::PriceSize,
                        tick_type: TickType::Ask,
                        value: TickValue::PriceSize {
                            price: ba.ask_price,
                            size: ba.ask_size as i64,
                        },
                        attrs: TickAttributes {
                            ask_past_high: ba.bid_ask_attribute.ask_past_high,
                            ..TickAttributes::default()
                        },
                        ts,
                    };
                    if tx.send(Arc::new(bid_tick)).is_err() {
                        break;
                    }
                    if tx.send(Arc::new(ask_tick)).is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    latch_error(
                        &last_error,
                        ib_error_to_market_data_error(&e, &symbol.symbol),
                    );
                    break;
                }
                None => break,
            }
        }
        let _ = ib_sub;
    })
}

fn spawn_tbt_midpoint_publisher(
    mut ib_sub: ibapi::subscriptions::Subscription<ibapi::market_data::realtime::MidPoint>,
    symbol: SymbolKey,
    req_id: ReqId,
    tx: broadcast::Sender<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    _line_guard: StreamingLineGuard,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match ib_sub.next().await {
                Some(Ok(mp)) => {
                    let ts = tr::offsetdatetime_to_chrono(mp.time);
                    let tick = Tick {
                        symbol: symbol.clone(),
                        req_id,
                        kind: TickKind::Price,
                        tick_type: TickType::Last,
                        value: TickValue::Price(mp.mid_point),
                        attrs: TickAttributes::default(),
                        ts,
                    };
                    if tx.send(Arc::new(tick)).is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    latch_error(
                        &last_error,
                        ib_error_to_market_data_error(&e, &symbol.symbol),
                    );
                    break;
                }
                None => break,
            }
        }
        let _ = ib_sub;
    })
}

fn spawn_rt_bar_publisher(
    mut ib_sub: ibapi::subscriptions::Subscription<ibapi::market_data::realtime::Bar>,
    symbol: SymbolKey,
    what_to_show: WhatToShow,
    tx: broadcast::Sender<Arc<Bar>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    _line_guard: StreamingLineGuard,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match ib_sub.next().await {
                Some(Ok(bar)) => {
                    let out = tr::translate_realtime_bar(&symbol, &bar, what_to_show);
                    if tx.send(Arc::new(out)).is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    latch_error(
                        &last_error,
                        ib_error_to_market_data_error(&e, &symbol.symbol),
                    );
                    break;
                }
                None => break,
            }
        }
        let _ = ib_sub;
    })
}

fn spawn_historical_publisher(
    mut ib_sub: ibapi::market_data::historical::HistoricalDataStreamingSubscription,
    symbol: SymbolKey,
    tf: Timeframe,
    tx: mpsc::Sender<HistoricalStreamEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Bound the initial collection loop so tests don't hang if
        // rust-ibapi never yields a `Historical` batch.
        let grace = Duration::from_secs(30);
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let next = tokio::time::timeout(
                std::cmp::max(remaining, Duration::from_millis(1)),
                ib_sub.next(),
            )
            .await;
            match next {
                Ok(Some(ibapi::market_data::historical::HistoricalBarUpdate::Historical(data))) => {
                    let bars = data
                        .bars
                        .iter()
                        .map(|b| tr::translate_historical_bar(&symbol, tf, b))
                        .collect::<Vec<_>>();
                    let first_ts = bars.first().map(|b| b.ts_open).unwrap_or_else(Utc::now);
                    let last_ts = bars.last().map(|b| b.ts_close).unwrap_or_else(Utc::now);
                    if tx
                        .send(HistoricalStreamEvent::Historical(bars))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if tx
                        .send(HistoricalStreamEvent::End { first_ts, last_ts })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    break;
                }
                Ok(Some(ibapi::market_data::historical::HistoricalBarUpdate::Update(bar))) => {
                    // Pre-initial-batch update: emit as Update too —
                    // rust-ibapi ordering is "Historical → End → Update",
                    // but we don't police that ordering strictly.
                    let out = tr::translate_historical_bar(&symbol, tf, &bar);
                    if tx.send(HistoricalStreamEvent::Update(out)).await.is_err() {
                        return;
                    }
                }
                Ok(Some(ibapi::market_data::historical::HistoricalBarUpdate::End {
                    start,
                    end,
                })) => {
                    let first_ts = tr::offsetdatetime_to_chrono(start);
                    let last_ts = tr::offsetdatetime_to_chrono(end);
                    let _ = tx
                        .send(HistoricalStreamEvent::End { first_ts, last_ts })
                        .await;
                    return;
                }
                Ok(None) | Err(_) => {
                    let _ = tx
                        .send(HistoricalStreamEvent::Error(MarketDataError::Other(
                            "historical stream closed before initial batch".into(),
                        )))
                        .await;
                    return;
                }
            }
        }
        // Continue draining updates until the handle is dropped (which
        // aborts this task).
        while let Some(next) = ib_sub.next().await {
            match next {
                ibapi::market_data::historical::HistoricalBarUpdate::Update(bar) => {
                    let out = tr::translate_historical_bar(&symbol, tf, &bar);
                    if tx.send(HistoricalStreamEvent::Update(out)).await.is_err() {
                        return;
                    }
                }
                ibapi::market_data::historical::HistoricalBarUpdate::End { .. } => return,
                ibapi::market_data::historical::HistoricalBarUpdate::Historical(_) => {
                    // IB should not re-emit Historical after End; drop.
                }
            }
        }
    })
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct construction with `port = 4001` and `allow_live = false`
    /// must be refused by `connect()` before any I/O is attempted.
    #[tokio::test]
    async fn connect_refuses_live_port_without_allow_live() {
        let cfg = IbMarketDataConfig {
            port: 4001,
            allow_live: false,
            ..IbMarketDataConfig::paper(7)
        };
        let mkt = IbMarketData::new(cfg);
        let err = mkt.connect().await.unwrap_err();
        assert!(
            matches!(err, MarketDataError::LiveTradingNotConfirmed),
            "expected LiveTradingNotConfirmed, got {err:?}"
        );
    }

    /// Defence-in-depth: a caller that starts from the safe
    /// `paper(..)` constructor and then mutates the port must still
    /// be rejected — the TOML `validate()` path never ran.
    #[tokio::test]
    async fn connect_refuses_post_construction_port_swap() {
        let mut cfg = IbMarketDataConfig::paper(7); // port 7497, safe
        cfg.port = 4001; // simulate programmatic mistake
        let mkt = IbMarketData::new(cfg);
        let err = mkt.connect().await.unwrap_err();
        assert!(
            matches!(err, MarketDataError::LiveTradingNotConfirmed),
            "expected LiveTradingNotConfirmed, got {err:?}"
        );
    }

    /// With `allow_live = true` the guard must NOT fire — the connect
    /// proceeds and eventually fails for a different reason (no real
    /// IB gateway at 127.0.0.1:4001).
    #[tokio::test]
    async fn connect_allows_live_port_when_confirmed() {
        let cfg = IbMarketDataConfig {
            port: 4001,
            allow_live: true,
            ib_op_timeout: Duration::from_millis(50),
            ..IbMarketDataConfig::default()
        };
        let mkt = IbMarketData::new(cfg);
        let err = mkt.connect().await.unwrap_err();
        // Any error OTHER than the guard is acceptable — we just
        // need proof that the guard did not short-circuit.
        assert!(
            !matches!(err, MarketDataError::LiveTradingNotConfirmed),
            "guard fired with allow_live = true; got {err:?}"
        );
    }

    // ── Slice B1: farm-up gate ───────────────────────────────────────────

    fn farm(code: FarmCode, connected: bool) -> FarmStatus {
        FarmStatus {
            code,
            connected,
            detail: String::new(),
        }
    }

    /// Happy path: the very first event is the MKT farm-up we want.
    #[tokio::test]
    async fn wait_for_mkt_farm_up_resolves_on_first_match() {
        let (tx, _orig_rx) = broadcast::channel::<FarmStatus>(16);
        let mut rx = tx.subscribe();
        tx.send(farm(FarmCode::MarketDataFarmOk, true)).unwrap();
        wait_for_mkt_farm_up(&mut rx).await.expect("farm-up");
    }

    /// The loop must skip unrelated farm events (HMDS, SecDef, MKT
    /// transitions that are NOT `Ok + connected`).
    #[tokio::test]
    async fn wait_for_mkt_farm_up_skips_unrelated_events() {
        let (tx, _orig_rx) = broadcast::channel::<FarmStatus>(16);
        let mut rx = tx.subscribe();
        // Noise first, then the match.
        tx.send(farm(FarmCode::HistoricalDataFarmOk, true)).unwrap();
        tx.send(farm(FarmCode::SecDefFarmOk, true)).unwrap();
        tx.send(farm(FarmCode::MarketDataFarmInactive, false))
            .unwrap();
        tx.send(farm(FarmCode::MarketDataFarmBroken, false))
            .unwrap();
        // MKT code but `connected = false` must NOT satisfy the gate.
        tx.send(farm(FarmCode::MarketDataFarmOk, false)).unwrap();
        tx.send(farm(FarmCode::MarketDataFarmOk, true)).unwrap();
        wait_for_mkt_farm_up(&mut rx).await.expect("farm-up");
    }

    /// Sender drop surfaces as an `Err`, not a hang.
    #[tokio::test]
    async fn wait_for_mkt_farm_up_errors_on_closed() {
        let (tx, _orig_rx) = broadcast::channel::<FarmStatus>(4);
        let mut rx = tx.subscribe();
        drop(tx); // close the channel
        let err = wait_for_mkt_farm_up(&mut rx).await.unwrap_err();
        assert!(
            matches!(err, MarketDataError::Other(ref s) if s.contains("farm-status")),
            "expected closed-channel error, got {err:?}"
        );
    }

    /// Core B1 behaviour: until the MKT farm-up event arrives, the
    /// gate must NOT return — and a `tokio::time` timeout is what the
    /// production code uses to bound the wait. `start_paused = true`
    /// lets us assert "no resolution for 500ms" without real sleep.
    #[tokio::test(start_paused = true)]
    async fn ready_waits_for_farm_up_mkt() {
        let (tx, _orig_rx) = broadcast::channel::<FarmStatus>(16);
        let mut rx = tx.subscribe();

        // Emulate the exact snippet used by `connect()`: wrap the
        // farm-up wait in a `with_ib_timeout` budget.
        let ib_timeout = Duration::from_secs(5);
        let gate = tokio::spawn(async move {
            with_ib_timeout(ib_timeout, FARM_UP_LABEL, wait_for_mkt_farm_up(&mut rx)).await
        });

        // Drive virtual time forward without any farm event — the
        // gate must remain unresolved.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !gate.is_finished(),
            "gate resolved before MKT farm-up was emitted"
        );

        // Now publish the expected farm-up; the gate must unblock.
        tx.send(farm(FarmCode::MarketDataFarmOk, true)).unwrap();
        // Yield the scheduler so the gate task can observe the event.
        tokio::task::yield_now().await;
        let res = tokio::time::timeout(Duration::from_secs(1), gate)
            .await
            .expect("gate did not complete after farm-up")
            .expect("gate task panicked");
        res.expect("gate future returned Err after farm-up");
    }

    /// Subscription-before-send ordering: if we subscribe BEFORE the
    /// farm-up is sent, the event is buffered and the gate still
    /// resolves. This is the invariant the `connect()` body relies on
    /// when it subscribes before awaiting `next_valid_order_id`.
    #[tokio::test(start_paused = true)]
    async fn farm_rx_subscribed_before_send_buffers_event() {
        let (tx, _orig_rx) = broadcast::channel::<FarmStatus>(16);
        // Subscribe FIRST …
        let mut rx = tx.subscribe();
        // … then a simulated "fast gateway" emits the farm-up before
        // the gate future is even polled.
        tx.send(farm(FarmCode::MarketDataFarmOk, true)).unwrap();
        // The buffered event must drive the gate to completion.
        wait_for_mkt_farm_up(&mut rx)
            .await
            .expect("buffered farm-up should satisfy gate");
    }
}
