//! Bracket-order submission helper (router-refactor slice 10).
//!
//! Wraps an [`OrderClient`] with the three-leg submission semantics
//! previously baked into `midas-broker::BrokerEngine::handle_create_bracket`.
//! The engine is being retired; this module owns the app-layer
//! equivalent so the IB transmit-last rule and cancel-fanout logic
//! stays encapsulated.
//!
//! # Semantics
//!
//! * Pre-allocate one IB order id per leg via [`OrderClient::next_order_id`].
//! * Place entry first with `transmit=false` if it has children,
//!   otherwise `transmit=true`.
//! * Place TP (if any) next. `transmit=true` only when SL is absent;
//!   otherwise `transmit=false`.
//! * Place SL (if any) last with `transmit=true` — this is the
//!   IB-mandated "parent transmits last" step that atomically
//!   activates the whole bracket.
//!
//! Any placement failure past the entry triggers a best-effort cancel
//! of already-submitted legs, mirroring the engine's behaviour.
//!
//! # Event translation
//!
//! The app previously consumed [`midas_broker::BrokerEvent`]s emitted by
//! the engine. The router surface emits [`OrderEvent`]s — the app-layer
//! bridge in `app/handlers.rs` maps those back to the existing
//! [`crate::app::Message`] variants so the UI handlers don't have to
//! change shape.

// S10a lands the helper in isolation; the call sites that consume
// every method / field move across in S10b (bracket submission) and
// S10c (OrderEvent subscription). Quiet the dead-code lints at module
// scope until those slices land.
#![allow(dead_code)]

use std::sync::Arc;

use midas_broker::{
    BracketParams, OrderAction, OrderClient, OrderError, OrderKind, OrderSpec, OrderType, Tif,
    TimeInForce,
};
use midas_broker_core::SymbolKey;
use uuid::Uuid;

/// Helper that packages `OrderClient::place_order` calls into a
/// three-leg bracket submission with IB-correct transmit semantics.
#[derive(Clone)]
pub struct BracketSubmitter {
    order_client: Arc<dyn OrderClient>,
}

/// Handle returned by [`BracketSubmitter::place_bracket`].
///
/// Callers correlate subsequent [`midas_broker::OrderEvent`]s back to
/// the submitted bracket via the `entry_id` / `tp_id` / `sl_id` IB
/// order ids. `parent_id` is a locally-minted UUID used by the UI
/// annotation-link map and the TickerState bracket projection.
#[derive(Debug, Clone)]
pub struct BracketHandle {
    /// Locally-generated bracket identity — distinct from IB ids.
    pub parent_id: Uuid,
    /// IB order id of the entry leg.
    pub entry_id: i32,
    /// IB order id of the take-profit leg, if the bracket has one.
    pub tp_id: Option<i32>,
    /// IB order id of the stop-loss leg, if the bracket has one.
    pub sl_id: Option<i32>,
}

impl BracketSubmitter {
    /// Build a submitter around an existing [`OrderClient`].
    pub fn new(order_client: Arc<dyn OrderClient>) -> Self {
        Self { order_client }
    }

    /// Access to the underlying [`OrderClient`]. Used by the app for
    /// ad-hoc cancel/modify on individual legs and for subscribing to
    /// the shared order-event stream.
    pub fn order_client(&self) -> &Arc<dyn OrderClient> {
        &self.order_client
    }

    /// Submit a bracket in IB-correct order (entry → TP → SL,
    /// transmit-last) and return the [`BracketHandle`] identifying
    /// every leg.
    pub async fn place_bracket(
        &self,
        params: BracketParams,
    ) -> Result<BracketHandle, OrderError> {
        let parent_id = Uuid::now_v7();
        let has_tp = params.take_profit.is_some();
        let has_sl = params.stop_loss.is_some();
        let has_children = has_tp || has_sl;

        // 1. Pre-allocate IB ids for every leg.
        let entry_id = self.order_client.next_order_id().await?;
        let tp_id = if has_tp {
            Some(self.order_client.next_order_id().await?)
        } else {
            None
        };
        let sl_id = if has_sl {
            Some(self.order_client.next_order_id().await?)
        } else {
            None
        };

        let symbol_key = SymbolKey {
            contract_id: params.con_id.unwrap_or(0),
            symbol: params.symbol.clone(),
        };

        // 2. Entry leg. Transmit=true only when we have no children
        //    (otherwise IB holds the parent until the last child
        //    transmits).
        let entry_spec = entry_spec(entry_id, &symbol_key, &params, !has_children);
        self.order_client.place_order(entry_spec).await?;

        // 3. Take-profit. Transmit-last only if SL is absent.
        if let Some(tp_id) = tp_id {
            let tp = params.take_profit.as_ref().expect("tp_id implies tp");
            let transmit = !has_sl;
            let spec = tp_spec(tp_id, entry_id, &symbol_key, &params, tp, transmit);
            if let Err(e) = self.order_client.place_order(spec).await {
                // Best-effort cancel of the orphan entry.
                let _ = self.order_client.cancel_order(entry_id, None).await;
                return Err(e);
            }
        }

        // 4. Stop-loss. Always transmit=true when present — triggers
        //    the whole bracket on the IB side.
        if let Some(sl_id) = sl_id {
            let sl = params.stop_loss.as_ref().expect("sl_id implies sl");
            let spec = sl_spec(sl_id, entry_id, &symbol_key, &params, sl, true);
            if let Err(e) = self.order_client.place_order(spec).await {
                let _ = self.order_client.cancel_order(entry_id, None).await;
                if let Some(tp_id) = tp_id {
                    let _ = self.order_client.cancel_order(tp_id, None).await;
                }
                return Err(e);
            }
        }

        Ok(BracketHandle {
            parent_id,
            entry_id,
            tp_id,
            sl_id,
        })
    }

    /// Cancel every working leg of a bracket.
    ///
    /// IB auto-cancels sibling children when one child fills (OCA), so
    /// callers typically pass only the ids they're still tracking.
    /// Cancel requests for unknown/terminal orders are tolerated by
    /// both the sim and the IB adapter.
    pub async fn cancel_bracket(&self, legs: &[i32]) -> Result<(), OrderError> {
        for &leg in legs {
            // Drop the stream immediately — we don't need the
            // per-cancel ack here; the main order_events subscription
            // reports the terminal Cancelled event.
            let _ = self.order_client.cancel_order(leg, None).await?;
        }
        Ok(())
    }

    /// Modify a single leg's price. Used for chart drag-to-modify on
    /// TP / SL lines.
    pub async fn modify_bracket_leg(
        &self,
        ib_order_id: i32,
        new_price: f64,
        is_stop: bool,
    ) -> Result<(), OrderError> {
        let modify = midas_broker::OrderModify {
            limit_price: if is_stop { None } else { Some(new_price) },
            stop_price: if is_stop { Some(new_price) } else { None },
            ..Default::default()
        };
        self.order_client.modify_order(ib_order_id, modify).await
    }
}

// ── OrderSpec builders ────────────────────────────────────────────────

fn entry_spec(
    ib_order_id: i32,
    symbol_key: &SymbolKey,
    params: &BracketParams,
    transmit: bool,
) -> OrderSpec {
    let order_type = kind_to_order_type(params.entry_kind);
    OrderSpec {
        ib_order_id,
        symbol: symbol_key.clone(),
        con_id: params.con_id.unwrap_or(0),
        action: params.action,
        order_type,
        quantity: params.quantity,
        limit_price: params.entry_price,
        stop_price: params.entry_stop_price,
        parent_id: None,
        transmit,
        tif: tif_from(None),
        outside_rth: params.outside_rth,
        oca_group: None,
        oca_type: None,
        conditions: Vec::new(),
        algo_strategy: None,
        algo_params: Vec::new(),
        good_after_time: None,
        good_till_date: None,
        display_size: None,
        hidden: false,
        trigger_method: midas_broker::TriggerMethod::Default,
        discretionary_amt: None,
        sweep_to_fill: false,
    }
}

fn tp_spec(
    ib_order_id: i32,
    parent_ib_id: i32,
    symbol_key: &SymbolKey,
    params: &BracketParams,
    tp: &midas_broker::TakeProfitParams,
    transmit: bool,
) -> OrderSpec {
    OrderSpec {
        ib_order_id,
        symbol: symbol_key.clone(),
        con_id: params.con_id.unwrap_or(0),
        action: opposite(params.action),
        order_type: OrderType::Limit,
        quantity: params.quantity,
        limit_price: Some(tp.price),
        stop_price: None,
        parent_id: Some(parent_ib_id),
        transmit,
        tif: tif_from(tp.tif),
        outside_rth: params.outside_rth,
        oca_group: None,
        oca_type: None,
        conditions: Vec::new(),
        algo_strategy: None,
        algo_params: Vec::new(),
        good_after_time: None,
        good_till_date: None,
        display_size: None,
        hidden: false,
        trigger_method: midas_broker::TriggerMethod::Default,
        discretionary_amt: None,
        sweep_to_fill: false,
    }
}

fn sl_spec(
    ib_order_id: i32,
    parent_ib_id: i32,
    symbol_key: &SymbolKey,
    params: &BracketParams,
    sl: &midas_broker::StopLossParams,
    transmit: bool,
) -> OrderSpec {
    let order_type = if sl.limit_price.is_some() {
        OrderType::StopLimit
    } else {
        OrderType::Stop
    };
    OrderSpec {
        ib_order_id,
        symbol: symbol_key.clone(),
        con_id: params.con_id.unwrap_or(0),
        action: opposite(params.action),
        order_type,
        quantity: params.quantity,
        limit_price: sl.limit_price,
        stop_price: Some(sl.stop_price),
        parent_id: Some(parent_ib_id),
        transmit,
        tif: tif_from(sl.tif),
        outside_rth: params.outside_rth,
        oca_group: None,
        oca_type: None,
        conditions: Vec::new(),
        algo_strategy: None,
        algo_params: Vec::new(),
        good_after_time: None,
        good_till_date: None,
        display_size: None,
        hidden: false,
        trigger_method: midas_broker::TriggerMethod::Default,
        discretionary_amt: None,
        sweep_to_fill: false,
    }
}

fn opposite(action: OrderAction) -> OrderAction {
    match action {
        OrderAction::Buy => OrderAction::Sell,
        OrderAction::Sell => OrderAction::Buy,
    }
}

fn kind_to_order_type(kind: OrderKind) -> OrderType {
    match kind {
        OrderKind::Market => OrderType::Market,
        OrderKind::Limit => OrderType::Limit,
        OrderKind::Stop => OrderType::Stop,
        OrderKind::StopLimit => OrderType::StopLimit,
        OrderKind::TrailingStop => OrderType::TrailingStop,
    }
}

fn tif_from(tif: Option<TimeInForce>) -> Tif {
    match tif.unwrap_or(TimeInForce::Gtc) {
        TimeInForce::Day => Tif::Day,
        TimeInForce::Gtc => Tif::Gtc,
        TimeInForce::Ioc => Tif::Ioc,
        TimeInForce::Gtd => Tif::Gtd,
        TimeInForce::Opg => Tif::Opg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_broker::sim::{SimConfig, SimOrderClient};
    use midas_broker::{
        BracketParams, OrderAction, OrderKind, SecurityType, StopLossParams, TakeProfitParams,
    };

    fn sim() -> Arc<SimOrderClient> {
        SimOrderClient::new(SimConfig::default().orders, None)
    }

    fn base_params() -> BracketParams {
        BracketParams {
            symbol: "AAPL".into(),
            con_id: Some(265598),
            sec_type: SecurityType::Stock,
            exchange: "SMART".into(),
            currency: "USD".into(),
            action: OrderAction::Buy,
            quantity: 100.0,
            outside_rth: false,
            entry_kind: OrderKind::Limit,
            entry_price: Some(150.0),
            entry_stop_price: None,
            take_profit: Some(TakeProfitParams {
                price: 160.0,
                tif: None,
            }),
            stop_loss: Some(StopLossParams {
                stop_price: 145.0,
                limit_price: None,
                tif: None,
            }),
            reference_price: Some(150.0),
            strategy: None,
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn full_bracket_allocates_three_ids() {
        let client: Arc<dyn OrderClient> = sim();
        let submitter = BracketSubmitter::new(client);
        let handle = submitter
            .place_bracket(base_params())
            .await
            .expect("bracket submission");
        assert!(handle.tp_id.is_some());
        assert!(handle.sl_id.is_some());
        assert_ne!(Some(handle.entry_id), handle.tp_id);
        assert_ne!(Some(handle.entry_id), handle.sl_id);
        assert_ne!(handle.tp_id, handle.sl_id);
    }

    #[tokio::test]
    async fn bracket_with_no_children_transmits_entry() {
        let client: Arc<dyn OrderClient> = sim();
        let submitter = BracketSubmitter::new(client);
        let mut p = base_params();
        p.take_profit = None;
        p.stop_loss = None;
        let handle = submitter.place_bracket(p).await.unwrap();
        assert!(handle.tp_id.is_none());
        assert!(handle.sl_id.is_none());
    }

    #[tokio::test]
    async fn bracket_with_only_tp() {
        let client: Arc<dyn OrderClient> = sim();
        let submitter = BracketSubmitter::new(client);
        let mut p = base_params();
        p.stop_loss = None;
        let handle = submitter.place_bracket(p).await.unwrap();
        assert!(handle.tp_id.is_some());
        assert!(handle.sl_id.is_none());
    }

    #[tokio::test]
    async fn bracket_with_only_sl() {
        let client: Arc<dyn OrderClient> = sim();
        let submitter = BracketSubmitter::new(client);
        let mut p = base_params();
        p.take_profit = None;
        let handle = submitter.place_bracket(p).await.unwrap();
        assert!(handle.tp_id.is_none());
        assert!(handle.sl_id.is_some());
    }

    #[tokio::test]
    async fn modify_leg_routes_limit_vs_stop() {
        let client: Arc<dyn OrderClient> = sim();
        let submitter = BracketSubmitter::new(client);
        let handle = submitter.place_bracket(base_params()).await.unwrap();
        // Modify TP (limit leg) — just verifies no error surfaces
        // from the sim for the happy path.
        submitter
            .modify_bracket_leg(handle.tp_id.unwrap(), 161.0, false)
            .await
            .unwrap();
        submitter
            .modify_bracket_leg(handle.sl_id.unwrap(), 144.0, true)
            .await
            .unwrap();
    }
}
