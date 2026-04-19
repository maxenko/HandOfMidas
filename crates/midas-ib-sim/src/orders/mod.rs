//! Order simulator. Stage 04 fills in the state machine, fill model, and
//! bracket semantics. Stage 01 ships the `OrderSimulator` trait + the
//! placeholder `BasicOrderSimulator` struct so the engine can name it.

pub mod brackets;
pub mod fill_model;
pub mod state_machine;

use crate::engine::types::{MarketSnapshot, OrderEmission, OrderId, PlaceOrderReq};

/// Public face of the order simulator — the engine calls `place` / `cancel`
/// on each client command, and feeds the result of every market update
/// through `on_market_snapshot` for fill evaluation.
pub trait OrderSimulator: Send {
    /// Admit a new order, returning the initial `OpenOrder` + `OrderStatus`
    /// emissions (Stage 04's fill pattern may schedule further events).
    fn place(&mut self, req: PlaceOrderReq) -> Vec<OrderEmission>;

    /// Cancel an order by id.
    fn cancel(&mut self, order_id: OrderId) -> Vec<OrderEmission>;

    /// Called after each market update. Evaluates resting orders for fills.
    fn on_market_snapshot(&mut self, snap: &MarketSnapshot) -> Vec<OrderEmission>;

    /// Emit the current open-orders list (used on `reqOpenOrders`).
    fn open_orders_snapshot(&self) -> Vec<OrderEmission>;
}

/// Stage-01 placeholder implementation. Stage 04 replaces the bodies.
#[derive(Default)]
pub struct BasicOrderSimulator {
    _priv: (),
}

impl BasicOrderSimulator {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl OrderSimulator for BasicOrderSimulator {
    fn place(&mut self, _req: PlaceOrderReq) -> Vec<OrderEmission> {
        todo!("Stage 04 — BasicOrderSimulator::place")
    }
    fn cancel(&mut self, _order_id: OrderId) -> Vec<OrderEmission> {
        todo!("Stage 04 — BasicOrderSimulator::cancel")
    }
    fn on_market_snapshot(&mut self, _snap: &MarketSnapshot) -> Vec<OrderEmission> {
        Vec::new()
    }
    fn open_orders_snapshot(&self) -> Vec<OrderEmission> {
        Vec::new()
    }
}
