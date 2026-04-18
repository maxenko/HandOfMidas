//! Per-leg order blotter — accumulates rows from `BrokerEvent` and
//! exposes a read-only view for the [`panel`] + grid renderer.
//!
//! # Shape
//!
//! One [`OrderRow`] per broker-assigned leg UUID. A three-leg bracket
//! produces three rows sharing the same `parent_id`. The
//! `by_parent` secondary index groups legs for sibling-column lookups
//! (`Take Profit` / `Stop Loss` columns in the UI reflect the sibling
//! leg's price).
//!
//! # Event semantics
//!
//! - `BracketCreated` — creates 1–3 rows. **Idempotent** on existing
//!   `parent_id`: immutable fields (`symbol`, `side`, `quantity`,
//!   `created_at`) are never overwritten. This is the v1 policy for
//!   late corrections — they're dropped, not merged. See plan's
//!   "Future growth" for conditional merge-upsert.
//! - `OrderStatusChanged` — **authoritative** source for cumulative
//!   `filled_qty`, `remaining_qty`, `avg_fill_price`, and `status`.
//! - `OrderFilled` — stamps `last_update_at` only. Does **not** mutate
//!   quantity fields; those come from `OrderStatusChanged`. Avoiding
//!   double-count on concurrent fills.
//! - `OrderCancelled` / `OrderRejected` — stamp status + `last_update_at`.
//!
//! All mutations bump the `generation` counter so iced views can
//! short-circuit re-renders via cheap equality on a `u64`.

pub mod columns;
pub mod panel;
pub mod persist;

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use midas_broker::BrokerEvent;
use midas_core::broker::{EntryKind, OrderAction, TimeInForce};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What role a leg plays inside its bracket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LegRole {
    /// The parent / entry order.
    Entry,
    /// Take-profit child.
    TakeProfit,
    /// Stop-loss child.
    StopLoss,
}

/// Order lifecycle status — a UI-friendly projection of the broker's
/// status string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Submitted to broker but not yet filled.
    Working,
    /// Partially filled; `filled_qty < quantity`.
    PartiallyFilled,
    /// Fully filled.
    Filled,
    /// Cancelled (by user, OCA, or engine).
    Cancelled,
    /// Rejected by the broker.
    Rejected,
}

impl OrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::PartiallyFilled => "Partial",
            Self::Filled => "Filled",
            Self::Cancelled => "Cancelled",
            Self::Rejected => "Rejected",
        }
    }

    /// Map a broker-reported status string (e.g. `"Submitted"`,
    /// `"Filled"`, `"Cancelled"`) to the UI projection. Broker strings
    /// are IB-style; unknown variants fall back to `Working` so rows
    /// never silently vanish.
    fn from_broker_str(s: &str) -> Self {
        match s {
            "Filled" => Self::Filled,
            "PartiallyFilled" | "PartialFilled" | "Partial" => Self::PartiallyFilled,
            "Cancelled" | "ApiCancelled" | "PendingCancel" => Self::Cancelled,
            "Rejected" | "Inactive" => Self::Rejected,
            _ => Self::Working,
        }
    }
}

/// A single row in the order blotter.
///
/// One row per broker-assigned leg UUID. Serde-serialised for redb
/// persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRow {
    pub order_id: Uuid,
    pub parent_id: Uuid,
    pub leg_role: LegRole,
    pub symbol: String,
    pub side: OrderAction,
    pub kind: EntryKind,
    pub quantity: f64,
    #[serde(default)]
    pub filled_qty: f64,
    #[serde(default)]
    pub remaining_qty: f64,
    #[serde(default)]
    pub avg_fill_price: Option<f64>,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    /// Sibling TP price — denormalised for the `Take Profit` column.
    pub tp_price: Option<f64>,
    /// Sibling SL price — denormalised for the `Stop Loss` column.
    pub sl_price: Option<f64>,
    pub status: OrderStatus,
    pub time_in_force: Option<TimeInForce>,
    pub ib_order_id: Option<i32>,
    pub ib_perm_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub last_update_at: DateTime<Utc>,
}

/// Live set of rows, keyed by broker-assigned leg UUID.
#[derive(Debug, Default)]
pub struct OrderBlotter {
    rows: BTreeMap<Uuid, OrderRow>,
    by_parent: HashMap<Uuid, Vec<Uuid>>,
    generation: u64,
}

impl OrderBlotter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current mutation generation. Slice-4 panels read this to decide
    /// whether to rebuild display rows.
    #[allow(dead_code)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Look up a single row. Reserved for Slice-4 per-row operations.
    #[allow(dead_code)]
    pub fn row(&self, id: Uuid) -> Option<&OrderRow> {
        self.rows.get(&id)
    }

    pub fn rows(&self) -> impl Iterator<Item = &OrderRow> {
        self.rows.values()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rehydrate from a persisted set of rows, e.g. on app startup.
    /// Rebuilds the secondary `by_parent` index.
    #[allow(dead_code)]
    pub fn hydrate(&mut self, rows: impl IntoIterator<Item = OrderRow>) {
        self.rows.clear();
        self.by_parent.clear();
        for row in rows {
            self.by_parent
                .entry(row.parent_id)
                .or_default()
                .push(row.order_id);
            self.rows.insert(row.order_id, row);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// Apply one broker event to the blotter. Returns the list of
    /// `order_id`s whose rows were touched — empty if the event was
    /// irrelevant or replayed. Callers forward each id into the
    /// persistence handle's `upsert`.
    pub fn apply(&mut self, event: &BrokerEvent) -> Vec<Uuid> {
        let touched: Vec<Uuid> = match event {
            BrokerEvent::BracketCreated {
                parent_id,
                take_profit_id,
                stop_loss_id,
                symbol,
                action,
                quantity,
                tp_price,
                sl_price,
                reference_price: _,
                entry_kind,
                entry_limit_price,
                entry_stop_price,
                sl_limit_price,
                tp_tif,
                sl_tif,
            } => self.apply_bracket_created(
                *parent_id,
                *take_profit_id,
                *stop_loss_id,
                symbol,
                *action,
                *quantity,
                *tp_price,
                *sl_price,
                *entry_kind,
                *entry_limit_price,
                *entry_stop_price,
                *sl_limit_price,
                *tp_tif,
                *sl_tif,
            ),

            BrokerEvent::OrderSubmitted {
                order_id,
                ib_order_id,
                ib_perm_id,
            } => self.apply_order_submitted(*order_id, *ib_order_id, *ib_perm_id),

            BrokerEvent::OrderStatusChanged {
                order_id,
                new_status,
                filled_qty,
                remaining_qty,
                avg_fill_price,
                ..
            } => self.apply_status_changed(
                *order_id,
                new_status,
                *filled_qty,
                *remaining_qty,
                *avg_fill_price,
            ),

            BrokerEvent::OrderFilled { order_id, .. } => self.apply_order_filled(*order_id),

            BrokerEvent::OrderCancelled { order_id, .. } => {
                self.apply_terminal_status(*order_id, OrderStatus::Cancelled)
            }

            BrokerEvent::OrderRejected { order_id, .. } => {
                self.apply_terminal_status(*order_id, OrderStatus::Rejected)
            }

            // All other variants are not blotter-relevant.
            _ => Vec::new(),
        };

        if !touched.is_empty() {
            self.generation = self.generation.wrapping_add(1);
        }
        touched
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_bracket_created(
        &mut self,
        parent_id: Uuid,
        tp_id: Option<Uuid>,
        sl_id: Option<Uuid>,
        symbol: &str,
        action: midas_broker::OrderAction,
        quantity: f64,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
        entry_kind: midas_broker::OrderKind,
        entry_limit_price: Option<f64>,
        entry_stop_price: Option<f64>,
        sl_limit_price: Option<f64>,
        tp_tif: Option<midas_broker::TimeInForce>,
        sl_tif: Option<midas_broker::TimeInForce>,
    ) -> Vec<Uuid> {
        // Idempotency: if we already have any row with this parent_id,
        // this is a replay — silently drop to preserve original
        // `created_at` and other immutable fields.
        if self.by_parent.contains_key(&parent_id) {
            return Vec::new();
        }

        let mut touched = Vec::with_capacity(3);

        let now = Utc::now();
        let side = translate_action(action);
        let kind = translate_kind(entry_kind);

        // Entry leg always exists.
        let entry_row = OrderRow {
            order_id: parent_id,
            parent_id,
            leg_role: LegRole::Entry,
            symbol: symbol.to_owned(),
            side,
            kind,
            quantity,
            filled_qty: 0.0,
            remaining_qty: quantity,
            avg_fill_price: None,
            limit_price: entry_limit_price,
            stop_price: entry_stop_price,
            tp_price,
            sl_price,
            status: OrderStatus::Working,
            time_in_force: None,
            ib_order_id: None,
            ib_perm_id: None,
            created_at: now,
            last_update_at: now,
        };
        self.rows.insert(parent_id, entry_row);
        self.by_parent.entry(parent_id).or_default().push(parent_id);
        touched.push(parent_id);

        // Opposite side for children: TP/SL close the position.
        let child_side = match side {
            OrderAction::Buy => OrderAction::Sell,
            OrderAction::Sell => OrderAction::Buy,
        };

        if let (Some(id), Some(price)) = (tp_id, tp_price) {
            let row = OrderRow {
                order_id: id,
                parent_id,
                leg_role: LegRole::TakeProfit,
                symbol: symbol.to_owned(),
                side: child_side,
                kind: EntryKind::Limit,
                quantity,
                filled_qty: 0.0,
                remaining_qty: quantity,
                avg_fill_price: None,
                limit_price: Some(price),
                stop_price: None,
                tp_price,
                sl_price,
                status: OrderStatus::Working,
                time_in_force: tp_tif.map(translate_tif),
                ib_order_id: None,
                ib_perm_id: None,
                created_at: now,
                last_update_at: now,
            };
            self.rows.insert(id, row);
            self.by_parent.entry(parent_id).or_default().push(id);
            touched.push(id);
        }

        if let (Some(id), Some(price)) = (sl_id, sl_price) {
            let sl_kind = if sl_limit_price.is_some() {
                EntryKind::StopLimit
            } else {
                EntryKind::Stop
            };
            let row = OrderRow {
                order_id: id,
                parent_id,
                leg_role: LegRole::StopLoss,
                symbol: symbol.to_owned(),
                side: child_side,
                kind: sl_kind,
                quantity,
                filled_qty: 0.0,
                remaining_qty: quantity,
                avg_fill_price: None,
                limit_price: sl_limit_price,
                stop_price: Some(price),
                tp_price,
                sl_price,
                status: OrderStatus::Working,
                time_in_force: sl_tif.map(translate_tif),
                ib_order_id: None,
                ib_perm_id: None,
                created_at: now,
                last_update_at: now,
            };
            self.rows.insert(id, row);
            self.by_parent.entry(parent_id).or_default().push(id);
            touched.push(id);
        }

        touched
    }

    fn apply_order_submitted(
        &mut self,
        order_id: Uuid,
        ib_order_id: i32,
        ib_perm_id: i64,
    ) -> Vec<Uuid> {
        let Some(row) = self.rows.get_mut(&order_id) else {
            return Vec::new();
        };
        row.ib_order_id = Some(ib_order_id);
        if ib_perm_id != 0 {
            row.ib_perm_id = Some(ib_perm_id);
        }
        row.last_update_at = Utc::now();
        vec![order_id]
    }

    fn apply_status_changed(
        &mut self,
        order_id: Uuid,
        new_status: &str,
        filled_qty: f64,
        remaining_qty: f64,
        avg_fill_price: f64,
    ) -> Vec<Uuid> {
        let Some(row) = self.rows.get_mut(&order_id) else {
            return Vec::new();
        };
        row.filled_qty = filled_qty;
        row.remaining_qty = remaining_qty;
        if avg_fill_price > 0.0 {
            row.avg_fill_price = Some(avg_fill_price);
        }
        row.status = OrderStatus::from_broker_str(new_status);
        row.last_update_at = Utc::now();
        vec![order_id]
    }

    fn apply_order_filled(&mut self, order_id: Uuid) -> Vec<Uuid> {
        // Per plan: OrderFilled is per-execution detail. Authoritative
        // cumulative fields are written by OrderStatusChanged. Here we
        // only refresh last_update_at so the row sorts correctly by
        // recency.
        let Some(row) = self.rows.get_mut(&order_id) else {
            return Vec::new();
        };
        row.last_update_at = Utc::now();
        vec![order_id]
    }

    fn apply_terminal_status(&mut self, order_id: Uuid, status: OrderStatus) -> Vec<Uuid> {
        let Some(row) = self.rows.get_mut(&order_id) else {
            return Vec::new();
        };
        row.status = status;
        row.last_update_at = Utc::now();
        vec![order_id]
    }
}

// ── Type translations ────────────────────────────────────────────────

fn translate_action(a: midas_broker::OrderAction) -> OrderAction {
    match a {
        midas_broker::OrderAction::Buy => OrderAction::Buy,
        midas_broker::OrderAction::Sell => OrderAction::Sell,
    }
}

fn translate_kind(k: midas_broker::OrderKind) -> EntryKind {
    match k {
        midas_broker::OrderKind::Market => EntryKind::Market,
        midas_broker::OrderKind::Limit => EntryKind::Limit,
        midas_broker::OrderKind::Stop => EntryKind::Stop,
        midas_broker::OrderKind::StopLimit => EntryKind::StopLimit,
        // TrailingStop has no midas-core mirror today; fall back to
        // Stop for display purposes. Future growth: add TrailingStop.
        midas_broker::OrderKind::TrailingStop => EntryKind::Stop,
    }
}

fn translate_tif(t: midas_broker::TimeInForce) -> TimeInForce {
    match t {
        midas_broker::TimeInForce::Day => TimeInForce::Day,
        midas_broker::TimeInForce::Gtc => TimeInForce::Gtc,
        midas_broker::TimeInForce::Ioc => TimeInForce::Ioc,
        midas_broker::TimeInForce::Gtd => TimeInForce::Gtd,
        midas_broker::TimeInForce::Opg => TimeInForce::Opg,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use midas_broker::OrderKind as BrokerKind;
    use midas_broker::{OrderAction as BrokerAction, TimeInForce as BrokerTif};

    // Deterministic-ish uuid for tests (no v4 feature on the uuid
    // dep; we fabricate unique ids from a process-local counter).
    fn rand_id() -> u128 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0x100);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        (n as u128) | (0x42_u128 << 96)
    }

    fn bracket_created(parent: Uuid, tp: Option<Uuid>, sl: Option<Uuid>) -> BrokerEvent {
        BrokerEvent::BracketCreated {
            parent_id: parent,
            take_profit_id: tp,
            stop_loss_id: sl,
            symbol: "AAPL".to_owned(),
            action: BrokerAction::Buy,
            quantity: 100.0,
            tp_price: tp.map(|_| 195.0),
            sl_price: sl.map(|_| 178.0),
            reference_price: Some(184.5),
            entry_kind: BrokerKind::Limit,
            entry_limit_price: Some(184.5),
            entry_stop_price: None,
            sl_limit_price: None,
            tp_tif: Some(BrokerTif::Day),
            sl_tif: Some(BrokerTif::Gtc),
        }
    }

    #[test]
    fn bracket_created_inserts_three_rows() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        let tp = Uuid::from_u128(rand_id());
        let sl = Uuid::from_u128(rand_id());
        assert!(!b
            .apply(&bracket_created(parent, Some(tp), Some(sl)))
            .is_empty());
        assert_eq!(b.len(), 3);

        let entry = b.row(parent).unwrap();
        assert_eq!(entry.leg_role, LegRole::Entry);
        assert_eq!(entry.kind, EntryKind::Limit);
        assert_eq!(entry.side, OrderAction::Buy);
        assert_eq!(entry.limit_price, Some(184.5));
        assert_eq!(entry.tp_price, Some(195.0));
        assert_eq!(entry.sl_price, Some(178.0));

        let tp_row = b.row(tp).unwrap();
        assert_eq!(tp_row.leg_role, LegRole::TakeProfit);
        assert_eq!(tp_row.kind, EntryKind::Limit);
        assert_eq!(tp_row.side, OrderAction::Sell);
        assert_eq!(tp_row.limit_price, Some(195.0));

        let sl_row = b.row(sl).unwrap();
        assert_eq!(sl_row.leg_role, LegRole::StopLoss);
        assert_eq!(sl_row.kind, EntryKind::Stop);
        assert_eq!(sl_row.stop_price, Some(178.0));
    }

    #[test]
    fn bracket_created_is_idempotent_on_replay() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        let tp = Uuid::from_u128(rand_id());
        let sl = Uuid::from_u128(rand_id());
        b.apply(&bracket_created(parent, Some(tp), Some(sl)));
        let gen1 = b.generation();
        let created_at_1 = b.row(parent).unwrap().created_at;

        // Replay the same event.
        let dirty = b.apply(&bracket_created(parent, Some(tp), Some(sl)));
        assert!(dirty.is_empty(), "replay must be a no-op");
        assert_eq!(b.generation(), gen1, "no generation bump on replay");
        assert_eq!(b.len(), 3, "no duplicate rows");
        assert_eq!(
            b.row(parent).unwrap().created_at,
            created_at_1,
            "created_at preserved on replay"
        );
    }

    #[test]
    fn order_status_changed_is_authoritative_for_cumulative() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        b.apply(&bracket_created(parent, None, None));

        let dirty = b.apply(&BrokerEvent::OrderStatusChanged {
            order_id: parent,
            old_status: "Submitted".to_owned(),
            new_status: "Filled".to_owned(),
            filled_qty: 100.0,
            remaining_qty: 0.0,
            avg_fill_price: 184.53,
        });
        assert!(!dirty.is_empty());

        let row = b.row(parent).unwrap();
        assert_eq!(row.filled_qty, 100.0);
        assert_eq!(row.remaining_qty, 0.0);
        assert_eq!(row.avg_fill_price, Some(184.53));
        assert_eq!(row.status, OrderStatus::Filled);
    }

    #[test]
    fn order_filled_does_not_change_qty_or_avg_fill() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        b.apply(&bracket_created(parent, None, None));
        // First set cumulative state via OrderStatusChanged.
        b.apply(&BrokerEvent::OrderStatusChanged {
            order_id: parent,
            old_status: "Submitted".to_owned(),
            new_status: "PartiallyFilled".to_owned(),
            filled_qty: 30.0,
            remaining_qty: 70.0,
            avg_fill_price: 184.50,
        });
        let qty_before = b.row(parent).unwrap().filled_qty;
        let avg_before = b.row(parent).unwrap().avg_fill_price;

        // OrderFilled arrives for the same execution — must NOT append.
        b.apply(&BrokerEvent::OrderFilled {
            order_id: parent,
            ib_exec_id: "exec-1".to_owned(),
            shares: 30.0,
            price: 184.50,
            commission: None,
        });

        let row = b.row(parent).unwrap();
        assert_eq!(
            row.filled_qty, qty_before,
            "OrderFilled must not touch filled_qty"
        );
        assert_eq!(
            row.avg_fill_price, avg_before,
            "OrderFilled must not touch avg_fill_price"
        );
    }

    #[test]
    fn cancelled_and_rejected_set_terminal_status() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        let tp = Uuid::from_u128(rand_id());
        b.apply(&bracket_created(parent, Some(tp), None));

        b.apply(&BrokerEvent::OrderCancelled {
            order_id: parent,
            reason: "user".to_owned(),
        });
        assert_eq!(b.row(parent).unwrap().status, OrderStatus::Cancelled);

        b.apply(&BrokerEvent::OrderRejected {
            order_id: tp,
            reason: "invalid".to_owned(),
        });
        assert_eq!(b.row(tp).unwrap().status, OrderStatus::Rejected);
    }

    #[test]
    fn unknown_order_id_is_no_op() {
        let mut b = OrderBlotter::new();
        let dirty = b.apply(&BrokerEvent::OrderStatusChanged {
            order_id: Uuid::from_u128(rand_id()),
            old_status: "Submitted".to_owned(),
            new_status: "Filled".to_owned(),
            filled_qty: 100.0,
            remaining_qty: 0.0,
            avg_fill_price: 184.53,
        });
        assert!(dirty.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn unrelated_events_are_no_op() {
        let mut b = OrderBlotter::new();
        let dirty = b.apply(&BrokerEvent::Connected { server_version: 12 });
        assert!(dirty.is_empty());
    }

    #[test]
    fn hydrate_rebuilds_rows_and_index() {
        let mut b = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        let now = Utc::now();
        let row = OrderRow {
            order_id: parent,
            parent_id: parent,
            leg_role: LegRole::Entry,
            symbol: "AAPL".to_owned(),
            side: OrderAction::Buy,
            kind: EntryKind::Limit,
            quantity: 100.0,
            filled_qty: 0.0,
            remaining_qty: 100.0,
            avg_fill_price: None,
            limit_price: Some(184.5),
            stop_price: None,
            tp_price: None,
            sl_price: None,
            status: OrderStatus::Working,
            time_in_force: None,
            ib_order_id: None,
            ib_perm_id: None,
            created_at: now,
            last_update_at: now,
        };
        b.hydrate([row]);
        assert_eq!(b.len(), 1);
        assert!(b.by_parent.contains_key(&parent));
    }
}
