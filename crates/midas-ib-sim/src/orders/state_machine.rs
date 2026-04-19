//! Order state-machine transitions. Stage 04 fills in.

use crate::engine::types::{OrderId, OrderKind, OrderStatusCode, Side};

#[derive(Clone, Debug)]
pub struct OrderRecord {
    pub order_id: OrderId,
    pub side: Side,
    pub kind: OrderKind,
    pub limit_price: Option<f64>,
    pub aux_price: Option<f64>,
    pub total_qty: f64,
    pub remaining_qty: f64,
    pub status: OrderStatusCode,
    pub stop_triggered: bool,
}
