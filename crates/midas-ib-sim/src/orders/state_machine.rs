//! Order state machine — tracks per-order status transitions.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Order state machine".

use midas_broker_core::ContractSpec;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{OrderId, OrderKind, OrderStatusCode, Side};

#[derive(Clone, Debug)]
pub struct OrderRecord {
    pub order_id: OrderId,
    pub contract: ContractSpec,
    pub side: Side,
    pub kind: OrderKind,
    pub limit_price: Option<f64>,
    pub aux_price: Option<f64>,
    pub total_qty: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub avg_fill_price: f64,
    pub status: OrderStatusCode,
    pub stop_triggered: bool,
    pub tif: String,
    pub account: String,
    pub parent_id: Option<OrderId>,
    pub oca_group: Option<String>,
    pub created_at: VirtualInstant,
}

impl OrderRecord {
    pub fn from_place_req(req: &crate::engine::types::PlaceOrderReq, now: VirtualInstant) -> Self {
        Self {
            order_id: req.order_id,
            contract: req.contract.clone(),
            side: req.side,
            kind: req.kind,
            limit_price: req.limit_price,
            aux_price: req.aux_price,
            total_qty: req.total_quantity,
            filled_qty: 0.0,
            remaining_qty: req.total_quantity,
            avg_fill_price: 0.0,
            status: OrderStatusCode::ApiPending,
            stop_triggered: false,
            tif: req.tif.clone(),
            account: req.account.clone(),
            parent_id: req.parent_id,
            oca_group: req.oca_group.clone(),
            created_at: now,
        }
    }

    pub fn is_terminal(status: OrderStatusCode) -> bool {
        matches!(
            status,
            OrderStatusCode::Filled | OrderStatusCode::Cancelled | OrderStatusCode::ApiCancelled
        )
    }

    pub fn is_filled(&self) -> bool {
        matches!(self.status, OrderStatusCode::Filled) && self.remaining_qty <= f64::EPSILON
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(
            self.status,
            OrderStatusCode::Cancelled | OrderStatusCode::ApiCancelled
        )
    }

    pub fn transition(&mut self, target: OrderStatusCode) -> bool {
        if !transition_allowed(self.kind, self.status, target) {
            return false;
        }
        self.status = target;
        true
    }
}

pub fn transition_allowed(kind: OrderKind, from: OrderStatusCode, to: OrderStatusCode) -> bool {
    use OrderStatusCode::*;

    if matches!(to, Cancelled | ApiCancelled) {
        return !matches!(from, Filled | Cancelled | ApiCancelled);
    }
    if matches!(to, Inactive) {
        return matches!(from, ApiPending);
    }
    if matches!(from, Inactive) {
        return matches!(to, PreSubmitted | Submitted);
    }

    match kind {
        OrderKind::Market => matches!(
            (from, to),
            (ApiPending, PendingSubmit)
                | (ApiPending | PendingSubmit, PreSubmitted)
                | (PreSubmitted, Submitted)
                | (PreSubmitted | Submitted, PartiallyFilled)
                | (PreSubmitted | Submitted | PartiallyFilled, Filled)
        ),
        OrderKind::Limit => matches!(
            (from, to),
            (ApiPending, PendingSubmit)
                | (ApiPending | PendingSubmit, PreSubmitted)
                | (PreSubmitted, Submitted)
                | (Submitted, PartiallyFilled)
                | (Submitted | PartiallyFilled, Filled)
                | (PreSubmitted, PartiallyFilled | Filled)
        ),
        OrderKind::Stop | OrderKind::StopLimit => matches!(
            (from, to),
            (ApiPending, PendingSubmit)
                | (ApiPending | PendingSubmit, PreSubmitted)
                | (PreSubmitted, Submitted)
                | (Submitted, PartiallyFilled)
                | (Submitted | PartiallyFilled, Filled)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrderKind::*;
    use OrderStatusCode::*;

    #[test]
    fn market_lifecycle_happy_path() {
        assert!(transition_allowed(Market, ApiPending, PreSubmitted));
        assert!(transition_allowed(Market, PreSubmitted, Filled));
    }

    #[test]
    fn limit_lifecycle_rests_at_submitted() {
        assert!(transition_allowed(Limit, ApiPending, PreSubmitted));
        assert!(transition_allowed(Limit, PreSubmitted, Submitted));
        assert!(transition_allowed(Limit, Submitted, Filled));
    }

    #[test]
    fn stop_lifecycle_through_presubmitted() {
        assert!(transition_allowed(Stop, ApiPending, PreSubmitted));
        assert!(transition_allowed(Stop, PreSubmitted, Submitted));
        assert!(transition_allowed(Stop, Submitted, Filled));
    }

    #[test]
    fn stop_limit_lifecycle() {
        assert!(transition_allowed(StopLimit, ApiPending, PreSubmitted));
        assert!(transition_allowed(StopLimit, PreSubmitted, Submitted));
        assert!(transition_allowed(StopLimit, Submitted, Filled));
    }

    #[test]
    fn cancel_always_allowed_from_non_terminal() {
        assert!(transition_allowed(Limit, PreSubmitted, Cancelled));
        assert!(transition_allowed(Market, Submitted, Cancelled));
        assert!(transition_allowed(Stop, Submitted, ApiCancelled));
    }

    #[test]
    fn cancel_forbidden_from_filled() {
        assert!(!transition_allowed(Market, Filled, Cancelled));
        assert!(!transition_allowed(Limit, Filled, Cancelled));
    }

    #[test]
    fn inactive_is_bracket_child_entry() {
        assert!(transition_allowed(Limit, ApiPending, Inactive));
        assert!(transition_allowed(Limit, Inactive, PreSubmitted));
        assert!(transition_allowed(Stop, Inactive, Submitted));
    }
}
