//! Synthetic fill model (pessimistic default). Stage 04 fills in.

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{OrderId, Side};

#[derive(Clone, Debug)]
pub struct Fill {
    pub order_id: OrderId,
    pub price: f64,
    pub shares: f64,
    pub ts: VirtualInstant,
    pub side: Side,
}
