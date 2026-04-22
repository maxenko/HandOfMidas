//! [`IbOrderClient`] — router-era IB order-placement adapter
//! (slice 4).
//!
//! Sibling to [`IbMarketData`](super::market_data::IbMarketData): both
//! share the same `Arc<ibapi::Client>` but present the narrower
//! [`OrderClient`](crate::OrderClient) surface. Lifecycle events from
//! `place_order` / `cancel_order` are translated and fanned out on
//! `order_events` / `position_events` / `account_events` broadcasts.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, mpsc, watch};

use crate::ib::market_data::IbMarketData;
use crate::ib::translation as tr;
use crate::order_client::{
    AccountEvent, CancelOrderEvent, CancelOrderStream, CompletedOrder, OpenOrder, OrderClient,
    OrderError, OrderEvent, OrderModify, OrderSpec, OrderType, PlaceOrderResult, PositionUpdate,
    Tif,
};
use crate::orders::state::OrderStatus;
use crate::orders::types::OrderAction;

// ───────────────────────────────────────────────────────────────────────────
// IbOrderClient
// ───────────────────────────────────────────────────────────────────────────

/// Router-era IB order adapter.
///
/// Holds a shared reference to the [`IbMarketData`] adapter (source of
/// the `Arc<ibapi::Client>` + `ordering_ready` watch) and three
/// broadcasts for order / position / account fan-out. Every
/// `place_order` call spawns a publisher task that drains the
/// rust-ibapi `Subscription<PlaceOrder>` and emits translated
/// [`OrderEvent`]s.
pub struct IbOrderClient {
    market: Arc<IbMarketData>,
    order_tx: broadcast::Sender<OrderEvent>,
    position_tx: broadcast::Sender<PositionUpdate>,
    account_tx: broadcast::Sender<AccountEvent>,
    ordering_ready: watch::Receiver<Option<i32>>,
}

impl IbOrderClient {
    /// Build a new order client on top of an existing market adapter.
    ///
    /// The market adapter owns the underlying rust-ibapi client and the
    /// `ordering_ready` watch; this struct just reads from them.
    pub fn new(market: Arc<IbMarketData>) -> Self {
        let (order_tx, _) = broadcast::channel(8192);
        let (position_tx, _) = broadcast::channel(4096);
        let (account_tx, _) = broadcast::channel(4096);
        let ordering_ready = market.ordering_ready();
        Self {
            market,
            order_tx,
            position_tx,
            account_tx,
            ordering_ready,
        }
    }

    /// Shared reference to the market adapter, for sites that need to
    /// reach the rust-ibapi client via the same handle.
    pub fn market(&self) -> &Arc<IbMarketData> {
        &self.market
    }

    async fn client(&self) -> Result<Arc<ibapi::Client>, OrderError> {
        self.market
            .client_handle()
            .await
            .ok_or(OrderError::Disconnected)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// OrderClient impl
// ───────────────────────────────────────────────────────────────────────────

#[async_trait]
impl OrderClient for IbOrderClient {
    async fn next_order_id(&self) -> Result<i32, OrderError> {
        // Wait for `ordering_ready` to carry `Some(_)`.
        let mut rx = self.ordering_ready.clone();
        loop {
            let snapshot = *rx.borrow();
            if let Some(id) = snapshot {
                // Advance the rust-ibapi client's counter and return
                // the current value atomically.
                let client = self.client().await?;
                return Ok(client
                    .next_valid_order_id()
                    .await
                    .map_err(|e| OrderError::Other(format!("next_valid_order_id: {e}")))
                    .unwrap_or(id));
            }
            // Block until the watch ticks.
            if rx.changed().await.is_err() {
                return Err(OrderError::Disconnected);
            }
        }
    }

    async fn place_order(&self, spec: OrderSpec) -> Result<PlaceOrderResult, OrderError> {
        let client = self.client().await?;
        let order_id = spec.ib_order_id;
        let contract = ibapi::contracts::Contract {
            contract_id: spec.con_id,
            symbol: spec.symbol.symbol.clone().into(),
            security_type: ibapi::contracts::SecurityType::Stock,
            exchange: "SMART".into(),
            currency: "USD".into(),
            ..ibapi::contracts::Contract::default()
        };
        let order = build_ib_order(&spec);
        let ib_sub = client
            .place_order(order_id, &contract, &order)
            .await
            .map_err(|e| OrderError::Other(format!("place_order: {e}")))?;
        let order_tx = self.order_tx.clone();
        let _ = spec;
        tokio::spawn(order_event_pump(ib_sub, order_id, order_tx));
        Ok(PlaceOrderResult {
            ib_order_id: order_id,
        })
    }

    async fn cancel_order(
        &self,
        ib_order_id: i32,
        manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError> {
        let client = self.client().await?;
        let stamp = manual_cancel_time
            .map(|t| t.format("%Y%m%d %H:%M:%S").to_string())
            .unwrap_or_default();
        let ib_sub = client
            .cancel_order(ib_order_id, &stamp)
            .await
            .map_err(|e| OrderError::Other(format!("cancel_order: {e}")))?;
        let (tx, rx) = mpsc::channel::<CancelOrderEvent>(16);
        let handle = tokio::spawn(cancel_event_pump(ib_sub, ib_order_id, tx));
        Ok(CancelOrderStream::new(rx, Box::new(move || handle.abort())))
    }

    async fn modify_order(&self, ib_order_id: i32, spec: OrderModify) -> Result<(), OrderError> {
        // IB has no dedicated "modify order" message — resending
        // place_order with the same id updates the working order.
        let client = self.client().await?;
        // Pull the current order off the open_orders snapshot so we can
        // build a replacement.
        let current = self
            .open_orders()
            .await?
            .into_iter()
            .find(|o| o.ib_order_id == ib_order_id)
            .ok_or(OrderError::NotFound(ib_order_id))?;
        let OrderModify {
            quantity,
            limit_price,
            stop_price,
            tif,
            outside_rth,
        } = spec;
        let updated = OrderSpec {
            ib_order_id,
            symbol: current.symbol.clone(),
            con_id: 0,
            action: current.action,
            order_type: current.order_type,
            quantity: quantity.unwrap_or(current.quantity),
            limit_price: limit_price.or(current.limit_price),
            stop_price: stop_price.or(current.stop_price),
            parent_id: current.parent_id,
            transmit: true,
            tif: tif.unwrap_or(current.tif),
            outside_rth: outside_rth.unwrap_or(false),
            oca_group: None,
            oca_type: None,
            conditions: vec![],
            algo_strategy: None,
            algo_params: vec![],
            good_after_time: None,
            good_till_date: None,
            display_size: None,
            hidden: false,
            trigger_method: crate::order_client::TriggerMethod::Default,
            discretionary_amt: None,
            sweep_to_fill: false,
        };
        let contract = ibapi::contracts::Contract {
            contract_id: updated.con_id,
            symbol: updated.symbol.symbol.clone().into(),
            security_type: ibapi::contracts::SecurityType::Stock,
            exchange: "SMART".into(),
            currency: "USD".into(),
            ..ibapi::contracts::Contract::default()
        };
        let order = build_ib_order(&updated);
        let ib_sub = client
            .place_order(ib_order_id, &contract, &order)
            .await
            .map_err(|e| OrderError::Other(format!("modify via place_order: {e}")))?;
        tokio::spawn(order_event_pump(ib_sub, ib_order_id, self.order_tx.clone()));
        Ok(())
    }

    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError> {
        let client = self.client().await?;
        let mut sub = client
            .all_open_orders()
            .await
            .map_err(|e| OrderError::Other(format!("all_open_orders: {e}")))?;
        let mut out = Vec::new();
        while let Some(r) = sub.next().await {
            match r {
                Ok(ibapi::orders::Orders::OrderData(od)) => out.push(translate_open_order(&od)),
                Ok(ibapi::orders::Orders::OrderStatus(_)) => {}
                Ok(ibapi::orders::Orders::Notice(_)) => {}
                Err(_) => break,
            }
        }
        Ok(out)
    }

    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError> {
        let client = self.client().await?;
        let mut sub = client
            .completed_orders(true)
            .await
            .map_err(|e| OrderError::Other(format!("completed_orders: {e}")))?;
        let mut out = Vec::new();
        while let Some(r) = sub.next().await {
            match r {
                Ok(ibapi::orders::Orders::OrderData(od)) => {
                    out.push(translate_completed_order(&od));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        Ok(out)
    }

    fn order_events(&self) -> broadcast::Receiver<OrderEvent> {
        self.order_tx.subscribe()
    }

    fn position_events(&self) -> broadcast::Receiver<PositionUpdate> {
        self.position_tx.subscribe()
    }

    fn account_events(&self) -> broadcast::Receiver<AccountEvent> {
        self.account_tx.subscribe()
    }

    fn name(&self) -> &str {
        "ib"
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Order event pump
// ───────────────────────────────────────────────────────────────────────────

async fn order_event_pump(
    mut sub: ibapi::subscriptions::Subscription<ibapi::orders::PlaceOrder>,
    order_id: i32,
    tx: broadcast::Sender<OrderEvent>,
) {
    let mut emitted_submitted = false;
    while let Some(next) = sub.next().await {
        match next {
            Ok(ibapi::orders::PlaceOrder::OrderStatus(st)) => {
                let canonical = translate_order_status(&st.status);
                if matches!(canonical, OrderStatus::Submitted) && !emitted_submitted {
                    emitted_submitted = true;
                    let _ = tx.send(OrderEvent::Submitted {
                        ib_order_id: order_id,
                    });
                }
                let _ = tx.send(OrderEvent::StatusChanged {
                    ib_order_id: st.order_id,
                    status: canonical,
                    filled: st.filled,
                    remaining: st.remaining,
                    avg_fill_price: st.average_fill_price,
                });
                // If the status is a terminal cancel, emit Cancelled too.
                if canonical == OrderStatus::Cancelled {
                    let _ = tx.send(OrderEvent::Cancelled {
                        ib_order_id: st.order_id,
                    });
                }
            }
            Ok(ibapi::orders::PlaceOrder::ExecutionData(ed)) => {
                let _ = tx.send(OrderEvent::ExecutionDetails {
                    ib_order_id: ed.execution.order_id,
                    exec_id: ed.execution.execution_id.clone(),
                    shares: ed.execution.shares,
                    price: ed.execution.price,
                });
            }
            Ok(ibapi::orders::PlaceOrder::CommissionReport(cr)) => {
                let _ = tx.send(OrderEvent::Commission {
                    exec_id: cr.execution_id.clone(),
                    commission: cr.commission,
                    realized_pnl: cr.realized_pnl,
                    currency: cr.currency.clone(),
                });
            }
            Ok(ibapi::orders::PlaceOrder::OpenOrder(_)) => {
                // Covered by open_orders() snapshot; no per-event dispatch.
            }
            Ok(ibapi::orders::PlaceOrder::Message(notice)) => {
                if notice.is_error() {
                    let _ = tx.send(OrderEvent::Rejected {
                        ib_order_id: order_id,
                        reason: notice.message.clone(),
                    });
                }
            }
            Err(_) => break,
        }
    }
    let _ = sub;
}

async fn cancel_event_pump(
    mut sub: ibapi::subscriptions::Subscription<ibapi::orders::CancelOrder>,
    order_id: i32,
    tx: mpsc::Sender<CancelOrderEvent>,
) {
    let _ = tx
        .send(CancelOrderEvent::Submitted {
            ib_order_id: order_id,
        })
        .await;
    while let Some(next) = sub.next().await {
        match next {
            Ok(ibapi::orders::CancelOrder::OrderStatus(st)) => {
                if translate_order_status(&st.status) == OrderStatus::Cancelled {
                    let _ = tx
                        .send(CancelOrderEvent::Cancelled {
                            ib_order_id: st.order_id,
                        })
                        .await;
                    return;
                }
            }
            Ok(ibapi::orders::CancelOrder::Notice(n)) => {
                if n.is_error() {
                    let _ = tx
                        .send(CancelOrderEvent::Error {
                            ib_order_id: order_id,
                            code: tr::translate_error_code(n.code),
                            message: n.message.clone(),
                        })
                        .await;
                    return;
                }
            }
            Err(_) => break,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Small translations used only here
// ───────────────────────────────────────────────────────────────────────────

fn build_ib_order(spec: &OrderSpec) -> ibapi::orders::Order {
    let action = match spec.action {
        OrderAction::Buy => ibapi::orders::Action::Buy,
        OrderAction::Sell => ibapi::orders::Action::Sell,
    };
    let tif = match spec.tif {
        Tif::Day => ibapi::orders::TimeInForce::Day,
        Tif::Gtc => ibapi::orders::TimeInForce::GoodTilCanceled,
        Tif::Ioc => ibapi::orders::TimeInForce::ImmediateOrCancel,
        Tif::Fok => ibapi::orders::TimeInForce::FillOrKill,
        Tif::Gtd => ibapi::orders::TimeInForce::GoodTilDate,
        Tif::Opg => ibapi::orders::TimeInForce::OnOpen,
        Tif::Dtc => ibapi::orders::TimeInForce::Day,
    };
    let mut order = ibapi::orders::Order {
        action,
        total_quantity: spec.quantity,
        order_type: order_type_to_ib_str(spec.order_type),
        limit_price: spec.limit_price,
        aux_price: spec.stop_price,
        tif,
        transmit: spec.transmit,
        outside_rth: spec.outside_rth,
        ..Default::default()
    };
    if let Some(pid) = spec.parent_id {
        order.parent_id = pid;
    }
    order
}

fn order_type_to_ib_str(t: OrderType) -> String {
    match t {
        OrderType::Market => "MKT",
        OrderType::Limit => "LMT",
        OrderType::Stop => "STP",
        OrderType::StopLimit => "STP LMT",
        OrderType::TrailingStop => "TRAIL",
        OrderType::MarketOnClose => "MOC",
        OrderType::LimitOnClose => "LOC",
        OrderType::MarketIfTouched => "MIT",
        OrderType::LimitIfTouched => "LIT",
        OrderType::PegToMarket => "PEG MKT",
        OrderType::PegToMidpoint => "PEG MID",
        OrderType::Relative => "REL",
        OrderType::Volatility => "VOL",
    }
    .to_string()
}

fn translate_order_status(s: &str) -> OrderStatus {
    match s {
        "PendingSubmit" | "ApiPending" => OrderStatus::PendingSubmit,
        "PreSubmitted" | "Submitted" => OrderStatus::Submitted,
        "Filled" => OrderStatus::Filled,
        "Cancelled" | "ApiCancelled" => OrderStatus::Cancelled,
        "Inactive" => OrderStatus::Rejected,
        _ => OrderStatus::PendingSubmit,
    }
}

fn tif_from_ib(t: &ibapi::orders::TimeInForce) -> Tif {
    match t {
        ibapi::orders::TimeInForce::Day => Tif::Day,
        ibapi::orders::TimeInForce::GoodTilCanceled => Tif::Gtc,
        ibapi::orders::TimeInForce::ImmediateOrCancel => Tif::Ioc,
        ibapi::orders::TimeInForce::FillOrKill => Tif::Fok,
        ibapi::orders::TimeInForce::GoodTilDate => Tif::Gtd,
        ibapi::orders::TimeInForce::OnOpen => Tif::Opg,
        _ => Tif::Day,
    }
}

fn order_type_from_str(s: &str) -> OrderType {
    match s {
        "MKT" => OrderType::Market,
        "LMT" => OrderType::Limit,
        "STP" => OrderType::Stop,
        "STP LMT" => OrderType::StopLimit,
        "TRAIL" => OrderType::TrailingStop,
        "MOC" => OrderType::MarketOnClose,
        "LOC" => OrderType::LimitOnClose,
        "MIT" => OrderType::MarketIfTouched,
        "LIT" => OrderType::LimitIfTouched,
        "PEG MKT" => OrderType::PegToMarket,
        "PEG MID" => OrderType::PegToMidpoint,
        "REL" => OrderType::Relative,
        "VOL" => OrderType::Volatility,
        _ => OrderType::Market,
    }
}

fn action_from_ib(a: &ibapi::orders::Action) -> OrderAction {
    match a {
        ibapi::orders::Action::Buy => OrderAction::Buy,
        _ => OrderAction::Sell,
    }
}

fn translate_open_order(od: &ibapi::orders::OrderData) -> OpenOrder {
    let sym = midas_broker_core::SymbolKey {
        contract_id: od.contract.contract_id,
        symbol: od.contract.symbol.to_string(),
    };
    OpenOrder {
        ib_order_id: od.order.order_id,
        perm_id: if od.order.perm_id != 0 {
            Some(od.order.perm_id as i64)
        } else {
            None
        },
        symbol: sym,
        action: action_from_ib(&od.order.action),
        order_type: order_type_from_str(&od.order.order_type),
        quantity: od.order.total_quantity,
        limit_price: od.order.limit_price,
        stop_price: od.order.aux_price,
        tif: tif_from_ib(&od.order.tif),
        status: translate_order_status(&od.order_state.status),
        filled: 0.0,
        remaining: od.order.total_quantity,
        avg_fill_price: None,
        parent_id: if od.order.parent_id != 0 {
            Some(od.order.parent_id)
        } else {
            None
        },
    }
}

fn translate_completed_order(od: &ibapi::orders::OrderData) -> CompletedOrder {
    let sym = midas_broker_core::SymbolKey {
        contract_id: od.contract.contract_id,
        symbol: od.contract.symbol.to_string(),
    };
    CompletedOrder {
        ib_order_id: od.order.order_id,
        perm_id: if od.order.perm_id != 0 {
            Some(od.order.perm_id as i64)
        } else {
            None
        },
        symbol: sym,
        action: action_from_ib(&od.order.action),
        order_type: order_type_from_str(&od.order.order_type),
        quantity: od.order.total_quantity,
        filled: od.order.total_quantity,
        avg_fill_price: None,
        status: translate_order_status(&od.order_state.status),
        completed_at: None,
    }
}
