//! Bracket parent-child semantics. Stage 04 fills in.

use crate::engine::types::OrderId;

/// Represents a parent order and its child TP / SL legs.
#[derive(Clone, Debug, Default)]
pub struct BracketGroup {
    pub parent: Option<OrderId>,
    pub take_profit: Option<OrderId>,
    pub stop_loss: Option<OrderId>,
    pub oca_group: Option<String>,
}
