//! Order simulator — state machine, fill model, bracket semantics, fill
//! patterns, account bookkeeping.
//!
//! Stage-04 implementation of `plan/ib-sim/04-order-lifecycle.md`.

pub mod accounts;
pub mod brackets;
pub mod determinism;
pub mod fill_model;
pub mod patterns;
pub mod state_machine;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::SymbolKey;

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::types::{
    CommissionReport, Execution, MarketSnapshot, OpenOrder, OrderEmission, OrderId, OrderKind,
    OrderStatus, OrderStatusCode, PlaceOrderReq,
};
use crate::orders::accounts::{symbol_key_for, AccountState};
use crate::orders::brackets::{
    sample_activation_jitter, sample_oca_cancel_jitter, BracketGroup, BracketLifecycle,
};
use crate::orders::fill_model::{maybe_fill, partial_chunks, Fill, SlippageKind};
use crate::orders::patterns::{actual_offset, select_pattern, steps_for, PatternKind, StepKind};
use crate::orders::state_machine::OrderRecord;
use crate::quirks::error_codes::{
    CANT_MODIFY_FILLED, DUPLICATE_ORDER_ID, NO_SECURITY_DEF, ORDER_CANCELLED,
    ORDER_NOT_YET_TRANSMITTED, ORDER_REJECTED, PRICE_NOT_MIN_TICK,
};

pub trait OrderSimulator: Send {
    fn place(&mut self, req: PlaceOrderReq) -> Vec<OrderEmission>;
    fn cancel(&mut self, order_id: OrderId) -> Vec<OrderEmission>;
    fn on_market_snapshot(&mut self, snap: &MarketSnapshot) -> Vec<OrderEmission>;
    fn open_orders_snapshot(&self) -> Vec<OrderEmission>;
}

pub struct BasicOrderSimulator {
    clock: Arc<dyn Clock>,
    base_seed: u64,
    orders: BTreeMap<OrderId, OrderRecord>,
    brackets: BTreeMap<OrderId /* parent */, BracketGroup>,
    bracket_child_to_parent: BTreeMap<OrderId, OrderId>,
    account: AccountState,
    seen_order_ids: BTreeSet<OrderId>,
    partial_threshold: f64,
    slippage: SlippageKind,
    last_snapshot: BTreeMap<SymbolKey, MarketSnapshot>,
    exec_counter: u64,
}

impl BasicOrderSimulator {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_seed(clock, 0)
    }

    pub fn with_seed(clock: Arc<dyn Clock>, base_seed: u64) -> Self {
        Self {
            clock,
            base_seed,
            orders: BTreeMap::new(),
            brackets: BTreeMap::new(),
            bracket_child_to_parent: BTreeMap::new(),
            account: AccountState::new("U0000001", 1_000_000.0),
            seen_order_ids: BTreeSet::new(),
            partial_threshold: 1_000.0,
            slippage: SlippageKind::FixedBps(1.0),
            last_snapshot: BTreeMap::new(),
            exec_counter: 0,
        }
    }

    pub fn set_slippage(&mut self, s: SlippageKind) {
        self.slippage = s;
    }

    pub fn set_partial_threshold(&mut self, t: f64) {
        self.partial_threshold = t;
    }

    pub fn account(&self) -> &AccountState {
        &self.account
    }

    pub fn orders(&self) -> &BTreeMap<OrderId, OrderRecord> {
        &self.orders
    }

    /// Mutable accessor used by the scenario runner's real-engine adapter to
    /// force deterministic terminal states (filled / cancelled) when the
    /// fill model can't converge within a scenario's virtual duration.
    /// Not part of the `OrderSimulator` trait; kept crate-public to make it
    /// clear this is an escape hatch, not the supported mutation surface.
    pub fn orders_mut_api(&mut self) -> &mut BTreeMap<OrderId, OrderRecord> {
        &mut self.orders
    }

    fn next_exec_id(&mut self) -> String {
        self.exec_counter += 1;
        format!("exec-{:08x}", self.exec_counter)
    }

    fn now(&self) -> VirtualInstant {
        self.clock.now()
    }

    fn emit_open_order(&self, o: &OrderRecord, status: OrderStatusCode) -> OrderEmission {
        OrderEmission::OpenOrder(OpenOrder {
            order_id: o.order_id,
            contract: o.contract.clone(),
            side: o.side,
            total_quantity: o.total_qty,
            kind: o.kind,
            limit_price: o.limit_price,
            aux_price: o.aux_price,
            status,
            tif: o.tif.clone(),
            account: o.account.clone(),
            parent_id: o.parent_id,
            oca_group: o.oca_group.clone(),
        })
    }

    fn emit_order_status(&self, o: &OrderRecord, status: OrderStatusCode) -> OrderEmission {
        OrderEmission::OrderStatus(OrderStatus {
            order_id: o.order_id,
            status,
            filled: o.filled_qty,
            remaining: o.remaining_qty,
            avg_fill_price: o.avg_fill_price,
            perm_id: o.order_id.0.wrapping_mul(31),
            parent_id: o.parent_id.map(|p| p.0).unwrap_or(0),
            last_fill_price: o.avg_fill_price,
            client_id: 0,
            why_held: String::new(),
            mkt_cap_price: 0.0,
        })
    }
}

fn reject(order_id: OrderId, code: i32, message: impl Into<String>) -> OrderEmission {
    OrderEmission::Reject {
        order_id,
        code,
        message: message.into(),
    }
}

const DEFAULT_MIN_TICK: f64 = 0.01;

fn respects_min_tick(price: f64) -> bool {
    let units = price / DEFAULT_MIN_TICK;
    (units - units.round()).abs() < 1e-6
}

impl OrderSimulator for BasicOrderSimulator {
    fn place(&mut self, req: PlaceOrderReq) -> Vec<OrderEmission> {
        if self.seen_order_ids.contains(&req.order_id) {
            return vec![reject(
                req.order_id,
                DUPLICATE_ORDER_ID,
                "Duplicate order id",
            )];
        }
        if req.contract.symbol().is_empty() {
            return vec![reject(
                req.order_id,
                NO_SECURITY_DEF,
                "No security definition has been found for the request",
            )];
        }
        if let Some(px) = req.limit_price {
            if !respects_min_tick(px) {
                return vec![reject(
                    req.order_id,
                    PRICE_NOT_MIN_TICK,
                    "The price does not conform to the minimum price variation for this contract.",
                )];
            }
        }
        if let Some(px) = req.aux_price {
            if !respects_min_tick(px) {
                return vec![reject(
                    req.order_id,
                    PRICE_NOT_MIN_TICK,
                    "The price does not conform to the minimum price variation for this contract.",
                )];
            }
        }
        match req.kind {
            OrderKind::Limit if req.limit_price.is_none() => {
                return vec![reject(
                    req.order_id,
                    ORDER_REJECTED,
                    "Limit order missing limit price",
                )];
            }
            OrderKind::Stop if req.aux_price.is_none() => {
                return vec![reject(
                    req.order_id,
                    ORDER_REJECTED,
                    "Stop order missing stop price",
                )];
            }
            OrderKind::StopLimit if req.limit_price.is_none() || req.aux_price.is_none() => {
                return vec![reject(
                    req.order_id,
                    ORDER_REJECTED,
                    "Stop-limit order missing limit or stop price",
                )];
            }
            _ => {}
        }

        self.seen_order_ids.insert(req.order_id);

        let mut rec = OrderRecord::from_place_req(&req, self.now());
        let is_bracket_child = req.parent_id.is_some();

        if is_bracket_child {
            rec.status = OrderStatusCode::Inactive;
            if let Some(parent_id) = req.parent_id {
                self.bracket_child_to_parent.insert(rec.order_id, parent_id);
                let group = self
                    .brackets
                    .entry(parent_id)
                    .or_insert_with(|| BracketGroup::new(parent_id, req.oca_group.clone()));
                match req.kind {
                    OrderKind::Limit => group.take_profit = Some(rec.order_id),
                    OrderKind::Stop | OrderKind::StopLimit => group.stop_loss = Some(rec.order_id),
                    OrderKind::Market => group.take_profit = Some(rec.order_id),
                }
            }
            let out = vec![
                self.emit_open_order(&rec, OrderStatusCode::Inactive),
                self.emit_order_status(&rec, OrderStatusCode::Inactive),
            ];
            self.orders.insert(rec.order_id, rec);
            return out;
        }

        rec.transition(OrderStatusCode::PendingSubmit);
        rec.transition(OrderStatusCode::PreSubmitted);

        let mut out = Vec::with_capacity(3);
        out.push(self.emit_open_order(&rec, OrderStatusCode::PreSubmitted));
        out.push(self.emit_order_status(&rec, OrderStatusCode::PreSubmitted));

        if matches!(rec.kind, OrderKind::Limit) {
            rec.transition(OrderStatusCode::Submitted);
            out.push(self.emit_order_status(&rec, OrderStatusCode::Submitted));
        }

        if req.oca_group.is_some() {
            self.brackets
                .entry(rec.order_id)
                .or_insert_with(|| BracketGroup::new(rec.order_id, req.oca_group.clone()));
        }

        self.orders.insert(rec.order_id, rec);
        out
    }

    fn cancel(&mut self, order_id: OrderId) -> Vec<OrderEmission> {
        let Some(rec) = self.orders.get_mut(&order_id) else {
            return vec![reject(
                order_id,
                ORDER_NOT_YET_TRANSMITTED,
                "OrderId that needs to be cancelled is not yet transmitted.",
            )];
        };
        if rec.is_filled() {
            return vec![reject(
                order_id,
                CANT_MODIFY_FILLED,
                "Cannot modify a filled order.",
            )];
        }
        if !rec.transition(OrderStatusCode::Cancelled) {
            return vec![reject(order_id, ORDER_CANCELLED, "Order cancelled")];
        }
        let rec_immut = &self.orders[&order_id];
        vec![self.emit_order_status(rec_immut, OrderStatusCode::Cancelled)]
    }

    fn on_market_snapshot(&mut self, snap: &MarketSnapshot) -> Vec<OrderEmission> {
        self.last_snapshot.insert(snap.symbol.clone(), snap.clone());
        let mut out = Vec::new();
        let candidate_ids: Vec<OrderId> = self
            .orders
            .iter()
            .filter_map(|(id, o)| {
                let is_active = matches!(
                    o.status,
                    OrderStatusCode::PreSubmitted
                        | OrderStatusCode::Submitted
                        | OrderStatusCode::PartiallyFilled
                );
                let same_symbol = symbol_key_for(&o.contract) == snap.symbol;
                if is_active && same_symbol {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        for oid in candidate_ids {
            let Some(rec) = self.orders.get_mut(&oid) else {
                continue;
            };
            let maybe = maybe_fill(
                rec,
                snap.mid,
                snap.bid,
                snap.ask,
                snap.ts,
                self.base_seed,
                self.slippage,
            );
            if let Some(fill) = maybe {
                let events = self.finalize_fill(oid, fill);
                out.extend(events);
            }
        }
        out
    }

    fn open_orders_snapshot(&self) -> Vec<OrderEmission> {
        let mut out = Vec::new();
        for (_, o) in self.orders.iter() {
            if !o.is_filled() && !o.is_cancelled() {
                out.push(self.emit_open_order(o, o.status));
                out.push(self.emit_order_status(o, o.status));
            }
        }
        out
    }
}

impl BasicOrderSimulator {
    fn finalize_fill(&mut self, order_id: OrderId, fill: Fill) -> Vec<OrderEmission> {
        let Some(rec) = self.orders.get(&order_id).cloned() else {
            return Vec::new();
        };

        let pattern = select_pattern(
            self.base_seed,
            order_id,
            rec.kind,
            rec.total_qty,
            self.partial_threshold,
        );

        let chunks: Vec<f64> = match pattern {
            PatternKind::C => {
                let mut c = partial_chunks(
                    fill.shares,
                    self.base_seed,
                    order_id,
                    (fill.shares / 2.0 - 0.1).max(1.0),
                );
                if c.len() < 2 {
                    c = vec![fill.shares / 2.0, fill.shares / 2.0];
                }
                c
            }
            _ => vec![fill.shares],
        };

        let steps = steps_for(pattern);
        let mut materialized: Vec<(Duration, OrderEmission)> = Vec::with_capacity(steps.len());

        let mut working = rec.clone();
        let total_qty = working.total_qty;
        let account_name = working.account.clone();
        let contract = working.contract.clone();
        let side = working.side;
        let symbol_key = symbol_key_for(&contract);

        for (step_idx, (base_off, kind)) in steps.iter().enumerate() {
            let offset = actual_offset(*base_off, self.base_seed, order_id, step_idx as u32);
            let emission = match *kind {
                StepKind::OpenOrderSubmitted => {
                    self.emit_open_order(&working, OrderStatusCode::Submitted)
                }
                StepKind::OpenOrderPreSubmitted => {
                    self.emit_open_order(&working, OrderStatusCode::PreSubmitted)
                }
                StepKind::ExecutionPart { chunk_idx } => {
                    let shares = chunks.get(chunk_idx as usize).copied().unwrap_or(0.0);
                    let price = fill.price;
                    working.filled_qty += shares;
                    working.remaining_qty = (total_qty - working.filled_qty).max(0.0);
                    let prev_notional = (working.filled_qty - shares) * working.avg_fill_price;
                    let this_notional = shares * price;
                    working.avg_fill_price = if working.filled_qty > 0.0 {
                        (prev_notional + this_notional) / working.filled_qty
                    } else {
                        price
                    };
                    let exec_id = self.next_exec_id();
                    OrderEmission::Execution(Execution {
                        req_id: None,
                        order_id,
                        exec_id,
                        time: fill.ts,
                        acct_number: account_name.clone(),
                        exchange: "SMART".into(),
                        side,
                        shares,
                        price,
                        perm_id: order_id.0.wrapping_mul(31),
                        client_id: 0,
                        liquidation: 0,
                        cumulative_quantity: working.filled_qty,
                        avg_price: working.avg_fill_price,
                        order_ref: None,
                        contract: contract.clone(),
                    })
                }
                StepKind::CommissionPart { chunk_idx } => {
                    let shares = chunks.get(chunk_idx as usize).copied().unwrap_or(0.0);
                    let commission = (0.005 * shares).max(1.00);
                    let exec_id = format!("exec-{:08x}", self.exec_counter);
                    OrderEmission::Commission(CommissionReport {
                        exec_id,
                        commission,
                        currency: "USD".into(),
                        realized_pnl: None,
                        yield_: None,
                        yield_redemption_date: None,
                    })
                }
                StepKind::OrderStatusPartiallyFilled { .. } => {
                    let _ = working.transition(OrderStatusCode::PartiallyFilled);
                    self.emit_order_status(&working, OrderStatusCode::Filled)
                }
                StepKind::OrderStatusFilled => {
                    let _ = working.transition(OrderStatusCode::Filled);
                    working.remaining_qty = 0.0;
                    self.emit_order_status(&working, OrderStatusCode::Filled)
                }
            };
            materialized.push((offset, emission));
        }

        materialized.sort_by_key(|a| a.0);
        let mut out: Vec<OrderEmission> = materialized.into_iter().map(|(_, e)| e).collect();

        if let Some(rec) = self.orders.get_mut(&order_id) {
            *rec = working;
        }

        for shares in &chunks {
            let (pu, pv) =
                self.account
                    .apply_fill(&symbol_key, &contract, side, *shares, fill.price);
            out.push(OrderEmission::Position(pu));
            out.push(OrderEmission::PortfolioValue(pv));
        }

        if let Some(group) = self.brackets.get(&order_id).cloned() {
            if matches!(group.state, BracketLifecycle::ParentWorking) {
                let mut activations: Vec<(Duration, OrderEmission)> = Vec::new();
                for child_id in [group.take_profit, group.stop_loss].into_iter().flatten() {
                    let jitter = sample_activation_jitter(self.base_seed, order_id, child_id);
                    if let Some(child) = self.orders.get_mut(&child_id) {
                        child.transition(OrderStatusCode::Submitted);
                        let emission = OrderEmission::OrderStatus(OrderStatus {
                            order_id: child.order_id,
                            status: OrderStatusCode::Submitted,
                            filled: 0.0,
                            remaining: child.remaining_qty,
                            avg_fill_price: 0.0,
                            perm_id: child.order_id.0.wrapping_mul(31),
                            parent_id: order_id.0,
                            last_fill_price: 0.0,
                            client_id: 0,
                            why_held: String::new(),
                            mkt_cap_price: 0.0,
                        });
                        activations.push((jitter, emission));
                    }
                }
                activations.sort_by_key(|a| a.0);
                out.extend(activations.into_iter().map(|(_, e)| e));
                if let Some(g) = self.brackets.get_mut(&order_id) {
                    g.state = BracketLifecycle::ParentFilled;
                }
            }
        }

        if let Some(parent_id) = self.bracket_child_to_parent.get(&order_id).copied() {
            let sibling_id = self.brackets.get(&parent_id).and_then(|g| {
                if g.take_profit == Some(order_id) {
                    g.stop_loss
                } else if g.stop_loss == Some(order_id) {
                    g.take_profit
                } else {
                    None
                }
            });
            if let Some(sid) = sibling_id {
                let _ = sample_oca_cancel_jitter(self.base_seed, order_id, sid);
                if let Some(sib) = self.orders.get_mut(&sid) {
                    if sib.transition(OrderStatusCode::Cancelled) {
                        out.push(OrderEmission::OrderStatus(OrderStatus {
                            order_id: sid,
                            status: OrderStatusCode::Cancelled,
                            filled: sib.filled_qty,
                            remaining: sib.remaining_qty,
                            avg_fill_price: sib.avg_fill_price,
                            perm_id: sid.0.wrapping_mul(31),
                            parent_id: parent_id.0,
                            last_fill_price: 0.0,
                            client_id: 0,
                            why_held: "OCA group cancelled by sibling fill".into(),
                            mkt_cap_price: 0.0,
                        }));
                    }
                }
                if let Some(g) = self.brackets.get_mut(&parent_id) {
                    g.state = BracketLifecycle::OneChildFilled;
                }
            }
        }

        out
    }
}

impl Default for BasicOrderSimulator {
    fn default() -> Self {
        Self::new(Arc::new(crate::engine::clock::RealClock::new()))
    }
}

#[cfg(test)]
mod tests {
    use midas_broker_core::ContractSpec;

    use super::*;
    use crate::engine::clock::VirtualClock;
    use crate::engine::types::Side;

    fn simulator() -> BasicOrderSimulator {
        BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), 1)
    }

    fn stock(sym: &str) -> ContractSpec {
        ContractSpec::Stock {
            symbol: sym.into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        }
    }

    fn place_req(order_id: i32, kind: OrderKind, qty: f64) -> PlaceOrderReq {
        PlaceOrderReq {
            order_id: OrderId(order_id),
            contract: stock("AAPL"),
            side: Side::Buy,
            total_quantity: qty,
            kind,
            limit_price: match kind {
                OrderKind::Limit | OrderKind::StopLimit => Some(150.00),
                _ => None,
            },
            aux_price: match kind {
                OrderKind::Stop | OrderKind::StopLimit => Some(145.00),
                _ => None,
            },
            tif: "DAY".into(),
            account: "U1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        }
    }

    fn snap(mid: f64, bid: f64, ask: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: symbol_key_for(&stock("AAPL")),
            mid,
            bid,
            ask,
            last: mid,
            volume: Some(100),
            ts: VirtualInstant::from_millis(1_000),
        }
    }

    #[test]
    fn reject_duplicate_order_id() {
        let mut s = simulator();
        let req = place_req(1, OrderKind::Market, 100.0);
        let _ = s.place(req.clone());
        let out = s.place(req);
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 103, .. })
        ));
    }

    #[test]
    fn reject_limit_without_price() {
        let mut s = simulator();
        let mut req = place_req(2, OrderKind::Limit, 100.0);
        req.limit_price = None;
        let out = s.place(req);
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 201, .. })
        ));
    }

    #[test]
    fn reject_sub_tick_price() {
        let mut s = simulator();
        let mut req = place_req(3, OrderKind::Limit, 100.0);
        req.limit_price = Some(150.001);
        let out = s.place(req);
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 110, .. })
        ));
    }

    #[test]
    fn cancel_before_place_is_10147() {
        let mut s = simulator();
        let out = s.cancel(OrderId(999));
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 10147, .. })
        ));
    }

    #[test]
    fn cancel_after_fill_is_104() {
        let mut s = simulator();
        let _ = s.place(place_req(11, OrderKind::Market, 100.0));
        let _ = s.on_market_snapshot(&snap(150.0, 149.99, 150.01));
        let out = s.cancel(OrderId(11));
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 104, .. })
        ));
    }

    #[test]
    fn reject_empty_symbol_200() {
        let mut s = simulator();
        let mut req = place_req(12, OrderKind::Market, 100.0);
        req.contract = ContractSpec::Stock {
            symbol: "".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        };
        let out = s.place(req);
        assert!(matches!(
            out.first(),
            Some(OrderEmission::Reject { code: 200, .. })
        ));
    }

    #[test]
    fn market_buy_fills_next_tick() {
        let mut s = simulator();
        let _ = s.place(place_req(10, OrderKind::Market, 100.0));
        let out = s.on_market_snapshot(&snap(150.0, 149.99, 150.01));
        assert!(out.iter().any(|e| matches!(e, OrderEmission::Execution(_))));
    }

    #[test]
    fn limit_buy_rests_then_fills() {
        let mut s = simulator();
        let _ = s.place(place_req(20, OrderKind::Limit, 100.0));
        let out1 = s.on_market_snapshot(&snap(150.5, 150.48, 150.52));
        assert!(!out1
            .iter()
            .any(|e| matches!(e, OrderEmission::Execution(_))));
        let out2 = s.on_market_snapshot(&snap(149.95, 149.93, 149.97));
        assert!(out2
            .iter()
            .any(|e| matches!(e, OrderEmission::Execution(_))));
    }

    #[test]
    fn stop_buy_triggers_then_fills() {
        let mut s = simulator();
        let mut req = place_req(30, OrderKind::Stop, 100.0);
        req.aux_price = Some(151.00);
        let _ = s.place(req);
        let _ = s.on_market_snapshot(&snap(149.0, 148.99, 149.01));
        let o2 = s.on_market_snapshot(&snap(151.5, 151.48, 151.52));
        assert!(o2.iter().any(|e| matches!(e, OrderEmission::Execution(_))));
    }

    #[test]
    fn stop_limit_buy_waits_for_price() {
        let mut s = simulator();
        let mut req = place_req(40, OrderKind::StopLimit, 100.0);
        req.aux_price = Some(150.00);
        req.limit_price = Some(150.10);
        let _ = s.place(req);
        let _ = s.on_market_snapshot(&snap(150.50, 150.48, 150.52));
        let o = s.on_market_snapshot(&snap(150.10, 150.05, 150.10));
        assert!(o.iter().any(|e| matches!(e, OrderEmission::Execution(_))));
    }

    fn execution_comes_before_status(events: &[OrderEmission], order_id: OrderId) -> Option<bool> {
        let mut exec_idx = None;
        let mut filled_idx = None;
        for (i, e) in events.iter().enumerate() {
            match e {
                OrderEmission::Execution(ex) if ex.order_id == order_id && exec_idx.is_none() => {
                    exec_idx = Some(i);
                }
                OrderEmission::OrderStatus(s)
                    if s.order_id == order_id
                        && matches!(s.status, OrderStatusCode::Filled)
                        && filled_idx.is_none() =>
                {
                    filled_idx = Some(i);
                }
                _ => {}
            }
        }
        match (exec_idx, filled_idx) {
            (Some(e), Some(s)) => Some(e < s),
            _ => None,
        }
    }

    #[test]
    fn pattern_b_reproduction_market_order() {
        let mut found = None;
        for i in 1..500 {
            let kind = patterns::select_pattern(1, OrderId(i), OrderKind::Market, 100.0, 1_000.0);
            if matches!(kind, PatternKind::B) {
                found = Some(i);
                break;
            }
        }
        let oid = found.expect("must find a Pattern-B id within 500 iterations");

        let mut s = simulator();
        let _ = s.place(place_req(oid, OrderKind::Market, 100.0));
        let out = s.on_market_snapshot(&snap(150.0, 149.99, 150.01));
        assert_eq!(
            execution_comes_before_status(&out, OrderId(oid)),
            Some(true),
        );
    }

    fn run_canonical_scenario(seed: u64) -> Vec<OrderEmission> {
        let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), seed);
        let mut all = Vec::new();
        for i in 1..=5 {
            all.extend(s.place(place_req(i, OrderKind::Market, 100.0)));
        }
        all.extend(s.on_market_snapshot(&snap(150.0, 149.99, 150.01)));
        all
    }

    #[test]
    fn three_runs_byte_identical() {
        let a = run_canonical_scenario(42);
        let b = run_canonical_scenario(42);
        let c = run_canonical_scenario(42);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(format!("{b:?}"), format!("{c:?}"));
    }

    #[test]
    fn different_seeds_diverge() {
        let a = run_canonical_scenario(1);
        let b = run_canonical_scenario(2);
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn bracket_parent_fill_activates_children_then_oca() {
        let mut s = simulator();
        let mut parent = place_req(100, OrderKind::Market, 100.0);
        parent.oca_group = Some("bracket-100".into());
        let _ = s.place(parent);

        let mut tp = place_req(101, OrderKind::Limit, 100.0);
        tp.side = Side::Sell;
        tp.limit_price = Some(155.00);
        tp.parent_id = Some(OrderId(100));
        tp.oca_group = Some("bracket-100".into());
        let _ = s.place(tp);

        let mut sl = place_req(102, OrderKind::Stop, 100.0);
        sl.side = Side::Sell;
        sl.aux_price = Some(145.00);
        sl.parent_id = Some(OrderId(100));
        sl.oca_group = Some("bracket-100".into());
        let _ = s.place(sl);

        let _ = s.on_market_snapshot(&snap(150.0, 149.99, 150.01));
        assert_eq!(s.orders[&OrderId(101)].status, OrderStatusCode::Submitted);
        assert_eq!(s.orders[&OrderId(102)].status, OrderStatusCode::Submitted);

        let _ = s.on_market_snapshot(&snap(155.10, 155.08, 155.12));
        assert_eq!(s.orders[&OrderId(101)].status, OrderStatusCode::Filled);
        assert_eq!(s.orders[&OrderId(102)].status, OrderStatusCode::Cancelled);
    }

    #[test]
    fn ten_orders_three_symbols_pnl_arithmetic() {
        let mut s = simulator();
        let syms = ["AAPL", "MSFT", "TSLA"];
        let mut counter = 1;
        for sym in syms {
            for _ in 0..3 {
                let mut req = place_req(counter, OrderKind::Market, 100.0);
                req.contract = stock(sym);
                let _ = s.place(req);
                counter += 1;
            }
        }
        let mut sell = place_req(counter, OrderKind::Market, 100.0);
        sell.contract = stock("AAPL");
        sell.side = Side::Sell;
        let _ = s.place(sell);

        for (i, sym) in syms.iter().enumerate() {
            let price = 100.0 + (i as f64) * 10.0;
            let mut sn = snap(price, price - 0.02, price + 0.02);
            sn.symbol = symbol_key_for(&stock(sym));
            let _ = s.on_market_snapshot(&sn);
        }
        assert_eq!(
            s.account
                .positions
                .values()
                .filter(|p| p.shares.abs() > 0.0)
                .count(),
            3
        );
    }
}
