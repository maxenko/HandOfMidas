//! Slice 2 contract tests: `MarketDataSource` and `OrderClient` must
//! remain object-safe (M-1) so the router can hold them as
//! `Arc<dyn _>`. The checks come in two layers:
//!
//! * Compile-only helpers (`_boxable`) prove the trait object types
//!   type-check.
//! * Mock impls confirm the trait surface is implementable end-to-end
//!   — a missing `Send`/`Sync` bound, a method that consumes `self`,
//!   or a stray `impl Trait` would break compilation here.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use midas_broker::order_client::PlaceOrderResult;
use midas_broker::stream::{HistoricalStream, RealtimeBarStream, TickStream};
use midas_broker::{
    AccountEvent, CancelOrderStream, CompletedOrder, HistoricalBarsResult, MarketDataSource,
    OpenOrder, OrderClient, OrderError, OrderEvent, OrderModify, OrderSpec, PositionUpdate,
};
use midas_broker_core::market_data::{
    ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration, MarketDataError,
    SecurityType, SymbolKey, TickByTickKind, Timeframe, WhatToShow,
};
use tokio::sync::{broadcast, mpsc, watch};

#[test]
fn market_data_source_is_object_safe() {
    // Compile-only: proves the trait can be stored as a boxed dyn.
    fn _boxable(_: Arc<dyn MarketDataSource>) {}
}

#[test]
fn order_client_is_object_safe() {
    fn _boxable(_: Arc<dyn OrderClient>) {}
}

// ─── MockSource: MarketDataSource ────────────────────────────────────────

struct MockSource {
    farm_tx: broadcast::Sender<FarmStatus>,
    connection_tx: watch::Sender<ConnectionState>,
}

impl MockSource {
    fn new() -> Self {
        let (farm_tx, _) = broadcast::channel(16);
        let (connection_tx, _) = watch::channel(ConnectionState::Disconnected);
        Self {
            farm_tx,
            connection_tx,
        }
    }
}

#[async_trait]
impl MarketDataSource for MockSource {
    async fn subscribe_ticks(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    async fn subscribe_tick_by_tick(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    async fn subscribe_realtime_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    async fn historical_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _end: DateTime<Utc>,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what_to_show: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    async fn historical_stream(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what_to_show: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    async fn resolve_contract(
        &self,
        _symbol: &SymbolKey,
        _sec_type: SecurityType,
        _exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        Err(MarketDataError::Other("mock".into()))
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.connection_tx.subscribe()
    }

    fn name(&self) -> &str {
        "mock"
    }
}

#[tokio::test]
async fn mock_market_data_source_compiles_and_boxes() {
    let m: Arc<dyn MarketDataSource> = Arc::new(MockSource::new());
    assert_eq!(m.name(), "mock");

    // Exercise every method through the trait object to lock in
    // object-safety across the full surface — a future non-dispatchable
    // method would surface here at compile time.
    let sym = SymbolKey {
        contract_id: 1,
        symbol: "MOCK".into(),
    };
    assert!(m
        .subscribe_ticks(&sym, 1, GenericTicks::new())
        .await
        .is_err());
    assert!(m
        .subscribe_tick_by_tick(&sym, 1, TickByTickKind::Last)
        .await
        .is_err());
    assert!(m
        .subscribe_realtime_bars(&sym, 1, WhatToShow::Trades)
        .await
        .is_err());
    assert!(m
        .historical_bars(
            &sym,
            1,
            Utc::now(),
            IbDuration::Days(1),
            Timeframe::M1,
            WhatToShow::Trades,
            true,
        )
        .await
        .is_err());
    assert!(m
        .historical_stream(
            &sym,
            1,
            IbDuration::Days(1),
            Timeframe::M1,
            WhatToShow::Trades,
            true,
        )
        .await
        .is_err());
    assert!(m
        .resolve_contract(&sym, SecurityType::Stock, "SMART")
        .await
        .is_err());
    let _ = m.farm_status();
    let _ = m.connection_state();
}

// ─── MockOrderClient: OrderClient ────────────────────────────────────────

struct MockOrderClient {
    events_tx: broadcast::Sender<OrderEvent>,
    positions_tx: broadcast::Sender<PositionUpdate>,
    accounts_tx: broadcast::Sender<AccountEvent>,
}

impl MockOrderClient {
    fn new() -> Self {
        let (events_tx, _) = broadcast::channel(16);
        let (positions_tx, _) = broadcast::channel(16);
        let (accounts_tx, _) = broadcast::channel(16);
        Self {
            events_tx,
            positions_tx,
            accounts_tx,
        }
    }
}

#[async_trait]
impl OrderClient for MockOrderClient {
    async fn next_order_id(&self) -> Result<i32, OrderError> {
        Ok(1)
    }

    async fn place_order(&self, _spec: OrderSpec) -> Result<PlaceOrderResult, OrderError> {
        Err(OrderError::Disconnected)
    }

    async fn cancel_order(
        &self,
        ib_order_id: i32,
        _manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError> {
        // Return a stream that closes immediately; the cancel closure
        // is a no-op for the mock but must still be non-null.
        let (_tx, rx) = mpsc::channel(1);
        Ok(CancelOrderStream::new(
            rx,
            Box::new(move || {
                let _ = ib_order_id;
            }),
        ))
    }

    async fn modify_order(&self, _ib_order_id: i32, _spec: OrderModify) -> Result<(), OrderError> {
        Ok(())
    }

    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError> {
        Ok(vec![])
    }

    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError> {
        Ok(vec![])
    }

    fn order_events(&self) -> broadcast::Receiver<OrderEvent> {
        self.events_tx.subscribe()
    }

    fn position_events(&self) -> broadcast::Receiver<PositionUpdate> {
        self.positions_tx.subscribe()
    }

    fn account_events(&self) -> broadcast::Receiver<AccountEvent> {
        self.accounts_tx.subscribe()
    }

    fn name(&self) -> &str {
        "mock"
    }
}

#[tokio::test]
async fn mock_order_client_compiles_and_boxes() {
    let m: Arc<dyn OrderClient> = Arc::new(MockOrderClient::new());
    assert_eq!(m.name(), "mock");
    assert_eq!(m.next_order_id().await.unwrap(), 1);
    assert!(m.open_orders().await.unwrap().is_empty());
    assert!(m.completed_orders().await.unwrap().is_empty());
    let _ = m.order_events();
    let _ = m.position_events();
    let _ = m.account_events();
}

// Suppress "unused" warnings on helper imports that are only here so the
// contract tests can observe the re-exports' names through the public API.
#[allow(dead_code)]
fn _sanity_last_error_carried(s: &TickStream) -> Option<&MarketDataError> {
    let _: &OnceLock<MarketDataError>;
    s.last_error()
}
