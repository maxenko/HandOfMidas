//! Tree-walking interpreter for scenario expressions.
//!
//! Evaluates an [`Expr`] against a [`ScenarioQuery`] snapshot. Non-fatal
//! mis-typings produce [`EvalError`]s the caller can render into scenario
//! failure messages.
//!
//! Supported surface (everything the canonical fixtures exercise):
//! - Paths: `orders[idx]`, `orders[order_ref]`, `orders[*].field`,
//!   `positions[SYMBOL].quantity`, `session[id].msg_count`
//! - Bare identifiers:
//!   - order-status names: `Filled`, `Cancelled`, `Submitted`, `PartiallyFilled`, …
//!   - booleans: `true`, `false` (already in the parser)
//!   - built-ins: `all_orders_have_terminal_status`, `no_orphan_bracket_children`,
//!     `session_duration`
//! - Functions: `sum(list)`, `count(list)`, `max(list|args…)`, `min(list|args…)`,
//!   `last_5s` (unary — returns a virtual-time window for future use; today
//!   the fixtures don't consume it directly, but `session[N].msg_count_last_5s`
//!   covers the same intent).
//! - Durations: the string `5min`, `30s`, `300s`, `00:05:00` — parsed here so
//!   `session_duration <= 5min` works.

use std::cmp::Ordering;
use std::time::Duration;

use super::ast::{BinOp, Expr, Index, Segment, Value};
use super::query::{OrderStatusName, ScenarioQuery};

/// Evaluation diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error("unknown identifier `{0}`")]
    UnknownIdent(String),

    #[error("unknown function `{0}` — valid: sum, count, max, min, last_5s")]
    UnknownFunction(String),

    #[error("function `{name}` expected {expected} argument(s), got {got}")]
    ArgCount {
        name: String,
        expected: &'static str,
        got: usize,
    },

    #[error("type error: expected {expected}, got {got}")]
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
    },

    #[error("path resolution failed at `{segment}`: {reason}")]
    PathResolve { segment: String, reason: String },

    #[error("comparison between incompatible types: {lhs} vs {rhs}")]
    IncomparableCompare {
        lhs: &'static str,
        rhs: &'static str,
    },

    #[error("duration parse error: `{0}`")]
    Duration(String),
}

/// Evaluate an expression in the context of a query.
pub fn eval(expr: &Expr, q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Path { root, segments } => resolve_path(root, segments, q),
        Expr::Call { name, args } => call_function(name, args, q),
        Expr::BinOp { op, lhs, rhs } => eval_binop(*op, lhs, rhs, q),
        Expr::And(lhs, rhs) => {
            let a = eval(lhs, q)?;
            let ab = coerce_bool(a)?;
            if !ab {
                return Ok(Value::Bool(false));
            }
            let b = eval(rhs, q)?;
            Ok(Value::Bool(coerce_bool(b)?))
        }
        Expr::Or(lhs, rhs) => {
            let a = eval(lhs, q)?;
            let ab = coerce_bool(a)?;
            if ab {
                return Ok(Value::Bool(true));
            }
            let b = eval(rhs, q)?;
            Ok(Value::Bool(coerce_bool(b)?))
        }
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

fn resolve_path(
    root: &str,
    segments: &[Segment],
    q: &dyn ScenarioQuery,
) -> Result<Value, EvalError> {
    if segments.is_empty() {
        // Bare identifier. Try (in order):
        //  1. status-name keywords → produce a string-typed Value
        //  2. built-in predicate names
        //  3. built-in value names (session_duration)
        //  4. bare duration literals (5min, 30s, 00:05:00)
        if let Some(status) = status_from_ident(root) {
            return Ok(Value::Str(status.as_str().to_string()));
        }
        if let Some(val) = builtin_value(root, q) {
            return Ok(val);
        }
        if let Ok(d) = parse_duration(root) {
            return Ok(Value::Num(d.as_secs_f64()));
        }
        return Err(EvalError::UnknownIdent(root.into()));
    }

    // Non-empty path — root is a namespace.
    match root {
        "orders" => resolve_orders(segments, q),
        "positions" => resolve_positions(segments, q),
        "session" => resolve_session(segments, q),
        other => Err(EvalError::UnknownIdent(other.into())),
    }
}

fn resolve_orders(segments: &[Segment], q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
    // First segment must be an index (either numeric, string/ident, or `*`).
    let Some((first, rest)) = segments.split_first() else {
        // `orders` alone — return a list of identifiers so `count(orders)` works.
        let ids: Vec<Value> = q
            .orders()
            .into_iter()
            .map(|o| Value::Str(o.order_ref))
            .collect();
        return Ok(Value::List(ids));
    };
    let Segment::Index(idx) = first else {
        return Err(EvalError::PathResolve {
            segment: "orders".into(),
            reason: "expected `[...]` after `orders`".into(),
        });
    };
    match idx {
        Index::Int(i) => {
            let snap = q
                .order_by_index(*i as usize)
                .ok_or_else(|| EvalError::PathResolve {
                    segment: format!("orders[{i}]"),
                    reason: "index out of range".into(),
                })?;
            order_field(&snap, rest)
        }
        Index::Str(s) | Index::Ident(s) => {
            let snap = q.order_by_ref(s).ok_or_else(|| EvalError::PathResolve {
                segment: format!("orders[{s}]"),
                reason: "no order with that order_ref".into(),
            })?;
            order_field(&snap, rest)
        }
        Index::Wildcard => {
            // Collect one Value per order at the requested tail field.
            let mut items = Vec::new();
            for snap in q.orders() {
                items.push(order_field(&snap, rest)?);
            }
            Ok(Value::List(items))
        }
    }
}

fn order_field(
    snap: &super::query::OrderSnapshot,
    segments: &[Segment],
) -> Result<Value, EvalError> {
    if segments.is_empty() {
        // Return the order_ref as a stable identity proxy — used by
        // `count(orders)` style but rarely in comparisons.
        return Ok(Value::Str(snap.order_ref.clone()));
    }
    let Segment::Field(name) = &segments[0] else {
        return Err(EvalError::PathResolve {
            segment: "<order>".into(),
            reason: "expected `.field` after an order index".into(),
        });
    };
    // Only one field depth today.
    if segments.len() > 1 {
        return Err(EvalError::PathResolve {
            segment: name.clone(),
            reason: "order fields are scalars — no nested access".into(),
        });
    }
    Ok(match name.as_str() {
        "order_ref" => Value::Str(snap.order_ref.clone()),
        "symbol" => Value::Str(snap.symbol.clone()),
        "side" => Value::Str(snap.side.clone()),
        "quantity" | "qty" => Value::Num(snap.quantity),
        "filled_qty" | "filled" => Value::Num(snap.filled_qty),
        "remaining_qty" | "remaining" => Value::Num(snap.remaining_qty),
        "status" => Value::Str(snap.status.as_str().into()),
        "limit_price" => snap.limit_price.map(Value::Num).unwrap_or(Value::Null),
        "stop_price" => snap.stop_price.map(Value::Num).unwrap_or(Value::Null),
        "avg_fill_price" => snap.avg_fill_price.map(Value::Num).unwrap_or(Value::Null),
        "parent_ref" => snap
            .parent_ref
            .as_deref()
            .map(|s| Value::Str(s.to_string()))
            .unwrap_or(Value::Null),
        other => {
            return Err(EvalError::PathResolve {
                segment: other.into(),
                reason: "unknown order field".into(),
            });
        }
    })
}

fn resolve_positions(segments: &[Segment], q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
    let Some((first, rest)) = segments.split_first() else {
        return Ok(Value::List(
            q.positions()
                .into_iter()
                .map(|p| Value::Str(p.symbol))
                .collect(),
        ));
    };
    let Segment::Index(idx) = first else {
        return Err(EvalError::PathResolve {
            segment: "positions".into(),
            reason: "expected `[SYMBOL]` after `positions`".into(),
        });
    };
    let key = match idx {
        Index::Ident(s) | Index::Str(s) => s.clone(),
        Index::Int(i) => i.to_string(),
        Index::Wildcard => {
            // Aggregate over positions at the chosen tail field.
            let mut items = Vec::new();
            for p in q.positions() {
                let fake = super::query::OrderSnapshot {
                    order_ref: p.symbol.clone(),
                    symbol: p.symbol.clone(),
                    side: String::new(),
                    quantity: p.quantity,
                    filled_qty: 0.0,
                    remaining_qty: 0.0,
                    status: OrderStatusName::Submitted,
                    limit_price: Some(p.avg_cost),
                    stop_price: None,
                    avg_fill_price: None,
                    parent_ref: None,
                };
                items.push(position_field(&fake, &p, rest)?);
            }
            return Ok(Value::List(items));
        }
    };
    let snap = q.position_for(&key).ok_or_else(|| EvalError::PathResolve {
        segment: format!("positions[{key}]"),
        reason: "no position for that symbol".into(),
    })?;
    // Reuse a plain field extractor.
    let dummy = super::query::OrderSnapshot {
        order_ref: String::new(),
        symbol: snap.symbol.clone(),
        side: String::new(),
        quantity: 0.0,
        filled_qty: 0.0,
        remaining_qty: 0.0,
        status: OrderStatusName::Submitted,
        limit_price: None,
        stop_price: None,
        avg_fill_price: None,
        parent_ref: None,
    };
    position_field(&dummy, &snap, rest)
}

fn position_field(
    _unused: &super::query::OrderSnapshot,
    pos: &super::query::PositionSnapshot,
    segments: &[Segment],
) -> Result<Value, EvalError> {
    if segments.is_empty() {
        // `positions[X]` alone — treat as quantity (matches the plan's
        // `positions[AAPL] == 100` shorthand).
        return Ok(Value::Num(pos.quantity));
    }
    let Segment::Field(name) = &segments[0] else {
        return Err(EvalError::PathResolve {
            segment: "<position>".into(),
            reason: "expected `.field`".into(),
        });
    };
    Ok(match name.as_str() {
        "symbol" => Value::Str(pos.symbol.clone()),
        "quantity" | "qty" => Value::Num(pos.quantity),
        "avg_cost" => Value::Num(pos.avg_cost),
        "realized_pnl" => Value::Num(pos.realized_pnl),
        "unrealized_pnl" => Value::Num(pos.unrealized_pnl),
        other => {
            return Err(EvalError::PathResolve {
                segment: other.into(),
                reason: "unknown position field".into(),
            });
        }
    })
}

fn resolve_session(segments: &[Segment], q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
    let Some((first, rest)) = segments.split_first() else {
        return Err(EvalError::PathResolve {
            segment: "session".into(),
            reason: "expected `[id]` after `session`".into(),
        });
    };
    let Segment::Index(idx) = first else {
        return Err(EvalError::PathResolve {
            segment: "session".into(),
            reason: "expected `[id]` after `session`".into(),
        });
    };
    let id = match idx {
        Index::Int(i) => *i as u64,
        Index::Str(s) | Index::Ident(s) => {
            s.parse::<u64>().map_err(|_| EvalError::PathResolve {
                segment: format!("session[{s}]"),
                reason: "session id must be integer".into(),
            })?
        }
        Index::Wildcard => {
            return Err(EvalError::PathResolve {
                segment: "session[*]".into(),
                reason: "wildcard not supported on sessions".into(),
            });
        }
    };
    let metrics = q
        .session_metrics(id)
        .ok_or_else(|| EvalError::PathResolve {
            segment: format!("session[{id}]"),
            reason: "no such session".into(),
        })?;
    if rest.is_empty() {
        return Ok(Value::Bool(metrics.connected));
    }
    let Segment::Field(name) = &rest[0] else {
        return Err(EvalError::PathResolve {
            segment: "<session>".into(),
            reason: "expected `.field`".into(),
        });
    };
    Ok(match name.as_str() {
        "msg_count" => Value::Num(metrics.msg_count as f64),
        "msg_count_last_5s" | "msg_count_since" => Value::Num(metrics.msg_count_last_5s as f64),
        "tick_count" => Value::Num(metrics.tick_count as f64),
        "connected" => Value::Bool(metrics.connected),
        other => {
            return Err(EvalError::PathResolve {
                segment: other.into(),
                reason: "unknown session field".into(),
            });
        }
    })
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

fn call_function(name: &str, args: &[Expr], q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
    let values: Vec<Value> = args.iter().map(|a| eval(a, q)).collect::<Result<_, _>>()?;
    match name {
        "sum" => fn_sum(&values, name),
        "count" => fn_count(&values, name),
        "max" => fn_extreme(&values, name, true),
        "min" => fn_extreme(&values, name, false),
        "last_5s" => fn_last_5s(&values, name),
        other => Err(EvalError::UnknownFunction(other.into())),
    }
}

fn flatten(values: &[Value]) -> Vec<Value> {
    // Single-list arg → flatten; multiple scalars → keep as-is.
    if values.len() == 1 {
        if let Value::List(items) = &values[0] {
            return items.clone();
        }
    }
    values.to_vec()
}

fn fn_sum(values: &[Value], name: &str) -> Result<Value, EvalError> {
    if values.is_empty() {
        return Err(EvalError::ArgCount {
            name: name.into(),
            expected: "≥1",
            got: 0,
        });
    }
    let flat = flatten(values);
    let mut acc = 0.0;
    for v in flat {
        match v {
            Value::Null => {} // skip absent fields
            Value::Num(n) => acc += n,
            Value::Int(i) => acc += i as f64,
            Value::Bool(b) => acc += if b { 1.0 } else { 0.0 },
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "number",
                    got: other.type_name(),
                });
            }
        }
    }
    Ok(Value::Num(acc))
}

fn fn_count(values: &[Value], name: &str) -> Result<Value, EvalError> {
    if values.is_empty() {
        return Err(EvalError::ArgCount {
            name: name.into(),
            expected: "1",
            got: 0,
        });
    }
    let flat = flatten(values);
    Ok(Value::Int(flat.len() as i64))
}

fn fn_extreme(values: &[Value], name: &str, want_max: bool) -> Result<Value, EvalError> {
    if values.is_empty() {
        return Err(EvalError::ArgCount {
            name: name.into(),
            expected: "≥1",
            got: 0,
        });
    }
    let flat = flatten(values);
    let mut best: Option<f64> = None;
    for v in flat {
        if matches!(v, Value::Null) {
            continue;
        }
        let n = v.as_f64().ok_or(EvalError::TypeMismatch {
            expected: "number",
            got: v.type_name(),
        })?;
        best = Some(match best {
            Some(b) if want_max => b.max(n),
            Some(b) => b.min(n),
            None => n,
        });
    }
    Ok(Value::Num(best.unwrap_or(0.0)))
}

fn fn_last_5s(values: &[Value], name: &str) -> Result<Value, EvalError> {
    // `last_5s` is a sentinel marker — returns a Value::Num encoding the
    // window width in seconds, so scenarios can chain it through comparisons
    // (`session[0].msg_count_since(last_5s) > 100`). Stage 06 keeps the
    // function arity at 0 — scenarios needing `since(…)` idioms should
    // use the pre-computed `session[N].msg_count_last_5s` field instead.
    if !values.is_empty() {
        return Err(EvalError::ArgCount {
            name: name.into(),
            expected: "0",
            got: values.len(),
        });
    }
    Ok(Value::Num(5.0))
}

// ---------------------------------------------------------------------------
// Binop
// ---------------------------------------------------------------------------

fn eval_binop(
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    q: &dyn ScenarioQuery,
) -> Result<Value, EvalError> {
    let l = eval(lhs, q)?;
    let r = eval(rhs, q)?;
    let result = match op {
        BinOp::Eq => l == r,
        BinOp::Neq => l != r,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = l.cmp_total(&r).ok_or(EvalError::IncomparableCompare {
                lhs: l.type_name(),
                rhs: r.type_name(),
            })?;
            match op {
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                BinOp::Ge => ord != Ordering::Less,
                _ => unreachable!(),
            }
        }
    };
    Ok(Value::Bool(result))
}

// ---------------------------------------------------------------------------
// Built-ins / keyword lookup
// ---------------------------------------------------------------------------

fn status_from_ident(name: &str) -> Option<OrderStatusName> {
    Some(match name {
        "ApiPending" => OrderStatusName::ApiPending,
        "PendingSubmit" => OrderStatusName::PendingSubmit,
        "PreSubmitted" => OrderStatusName::PreSubmitted,
        "Submitted" => OrderStatusName::Submitted,
        "Filled" => OrderStatusName::Filled,
        "PartiallyFilled" => OrderStatusName::PartiallyFilled,
        "Cancelled" => OrderStatusName::Cancelled,
        "ApiCancelled" => OrderStatusName::ApiCancelled,
        "Inactive" => OrderStatusName::Inactive,
        _ => return None,
    })
}

fn builtin_value(name: &str, q: &dyn ScenarioQuery) -> Option<Value> {
    match name {
        "all_orders_have_terminal_status" => {
            let all = q.orders().iter().all(|o| o.status.is_terminal());
            Some(Value::Bool(all))
        }
        "no_orphan_bracket_children" => {
            // A bracket child is orphaned if its parent_ref points to an
            // order that no longer exists (or isn't terminal cleanly).
            let all = q.orders();
            let orphans = all.iter().any(|o| {
                if let Some(pref) = &o.parent_ref {
                    !all.iter().any(|p| &p.order_ref == pref)
                } else {
                    false
                }
            });
            Some(Value::Bool(!orphans))
        }
        "session_duration" => Some(Value::Num(q.session_duration().as_secs_f64())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Duration parsing — `5min`, `30s`, `1500ms`, `00:05:00`.
// ---------------------------------------------------------------------------

pub fn parse_duration(s: &str) -> Result<Duration, EvalError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(EvalError::Duration(s.into()));
    }
    // `HH:MM:SS`
    if t.matches(':').count() == 2 {
        let parts: Vec<_> = t.split(':').collect();
        let h: u64 = parts[0]
            .parse()
            .map_err(|_| EvalError::Duration(s.into()))?;
        let m: u64 = parts[1]
            .parse()
            .map_err(|_| EvalError::Duration(s.into()))?;
        let sec: u64 = parts[2]
            .parse()
            .map_err(|_| EvalError::Duration(s.into()))?;
        return Ok(Duration::from_secs(h * 3600 + m * 60 + sec));
    }
    // Numeric + suffix.
    let (num_end, suffix) = t
        .char_indices()
        .find(|(_, c)| c.is_alphabetic())
        .map(|(i, _)| (i, &t[i..]))
        .unwrap_or((t.len(), ""));
    let num_str = &t[..num_end];
    if num_str.is_empty() {
        return Err(EvalError::Duration(s.into()));
    }
    let n: f64 = num_str.parse().map_err(|_| EvalError::Duration(s.into()))?;
    let d = match suffix {
        "" | "s" | "sec" | "secs" => Duration::from_secs_f64(n),
        "ms" => Duration::from_millis(n as u64),
        "min" | "m" => Duration::from_secs_f64(n * 60.0),
        "h" | "hr" | "hour" | "hours" => Duration::from_secs_f64(n * 3600.0),
        other => return Err(EvalError::Duration(format!("unknown unit `{other}`"))),
    };
    Ok(d)
}

fn coerce_bool(v: Value) -> Result<bool, EvalError> {
    v.as_bool().ok_or(EvalError::TypeMismatch {
        expected: "bool",
        got: v.type_name(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::parser::parse;
    use super::super::query::{
        OrderSnapshot, OrderStatusName, PositionSnapshot, ScenarioQuery, SessionMetrics,
    };
    use super::*;

    #[derive(Default)]
    struct MockQ {
        orders: Vec<OrderSnapshot>,
        positions: Vec<PositionSnapshot>,
        sessions: std::collections::BTreeMap<u64, SessionMetrics>,
        duration: Duration,
    }

    impl ScenarioQuery for MockQ {
        fn orders(&self) -> Vec<OrderSnapshot> {
            self.orders.clone()
        }
        fn position_for(&self, symbol: &str) -> Option<PositionSnapshot> {
            self.positions.iter().find(|p| p.symbol == symbol).cloned()
        }
        fn positions(&self) -> Vec<PositionSnapshot> {
            self.positions.clone()
        }
        fn session_metrics(&self, id: u64) -> Option<SessionMetrics> {
            self.sessions.get(&id).cloned()
        }
        fn session_duration(&self) -> Duration {
            self.duration
        }
    }

    fn order(order_ref: &str, status: OrderStatusName, filled: f64) -> OrderSnapshot {
        OrderSnapshot {
            order_ref: order_ref.into(),
            symbol: "AAPL".into(),
            side: "buy".into(),
            quantity: 100.0,
            filled_qty: filled,
            remaining_qty: 100.0 - filled,
            status,
            limit_price: None,
            stop_price: None,
            avg_fill_price: Some(175.5),
            parent_ref: None,
        }
    }

    fn eval_str(src: &str, q: &dyn ScenarioQuery) -> Result<Value, EvalError> {
        let ast = parse(src).unwrap();
        eval(&ast, q)
    }

    #[test]
    fn literal_comparisons() {
        let q = MockQ::default();
        assert_eq!(eval_str("1 == 1", &q).unwrap(), Value::Bool(true));
        assert_eq!(eval_str("1 < 2", &q).unwrap(), Value::Bool(true));
        assert_eq!(eval_str("2 >= 2", &q).unwrap(), Value::Bool(true));
    }

    #[test]
    fn order_status_by_index() {
        let mut q = MockQ::default();
        q.orders.push(order("o1", OrderStatusName::Filled, 100.0));
        assert_eq!(
            eval_str("orders[0].status == Filled", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn order_status_by_ref_bare_ident() {
        let mut q = MockQ::default();
        q.orders
            .push(order("smoke-1", OrderStatusName::Filled, 100.0));
        assert_eq!(
            eval_str("orders[smoke-1].status == Filled", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn order_status_by_ref_string() {
        let mut q = MockQ::default();
        q.orders
            .push(order("smoke-1", OrderStatusName::Filled, 100.0));
        assert_eq!(
            eval_str("orders[\"smoke-1\"].status == Filled", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn sum_of_filled_qty() {
        let mut q = MockQ::default();
        q.orders.push(order("a", OrderStatusName::Filled, 60.0));
        q.orders.push(order("b", OrderStatusName::Filled, 40.0));
        assert_eq!(
            eval_str("sum(orders[*].filled_qty) == 100", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn count_orders() {
        let mut q = MockQ::default();
        q.orders.push(order("a", OrderStatusName::Filled, 10.0));
        q.orders.push(order("b", OrderStatusName::Submitted, 0.0));
        q.orders.push(order("c", OrderStatusName::Cancelled, 0.0));
        assert_eq!(
            eval_str("count(orders[*].status) == 3", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn max_min_aggregates() {
        let mut q = MockQ::default();
        q.orders.push(order("a", OrderStatusName::Filled, 10.0));
        q.orders.push(order("b", OrderStatusName::Filled, 50.0));
        q.orders.push(order("c", OrderStatusName::Filled, 30.0));
        assert_eq!(
            eval_str("max(orders[*].filled_qty)", &q).unwrap(),
            Value::Num(50.0)
        );
        assert_eq!(
            eval_str("min(orders[*].filled_qty)", &q).unwrap(),
            Value::Num(10.0)
        );
    }

    #[test]
    fn positions_by_symbol_scalar() {
        let mut q = MockQ::default();
        q.positions.push(PositionSnapshot {
            symbol: "AAPL".into(),
            quantity: 100.0,
            avg_cost: 175.0,
            realized_pnl: 0.0,
            unrealized_pnl: 0.0,
        });
        // `positions[AAPL] == 100` shorthand — treats value as quantity.
        assert_eq!(
            eval_str("positions[AAPL] == 100", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn positions_field_access() {
        let mut q = MockQ::default();
        q.positions.push(PositionSnapshot {
            symbol: "AAPL".into(),
            quantity: 100.0,
            avg_cost: 175.0,
            realized_pnl: 0.0,
            unrealized_pnl: 0.0,
        });
        assert_eq!(
            eval_str("positions[AAPL].quantity > 0", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn session_metrics() {
        let mut q = MockQ::default();
        q.sessions.insert(
            0,
            SessionMetrics {
                msg_count: 42,
                msg_count_last_5s: 101,
                tick_count: 0,
                connected: true,
            },
        );
        assert_eq!(
            eval_str("session[0].msg_count_last_5s > 100", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn short_circuit_and() {
        // rhs would error (unknown ident) but AND short-circuits.
        let q = MockQ::default();
        assert_eq!(
            eval_str("false && nothing_here", &q).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn short_circuit_or() {
        let q = MockQ::default();
        assert_eq!(
            eval_str("true || nothing_here", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn all_orders_have_terminal_status_builtin() {
        let mut q = MockQ::default();
        q.orders.push(order("a", OrderStatusName::Filled, 100.0));
        q.orders.push(order("b", OrderStatusName::Cancelled, 0.0));
        assert_eq!(
            eval_str("all_orders_have_terminal_status", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn session_duration_vs_5min() {
        let mut q = MockQ {
            duration: Duration::from_secs(200),
            ..Default::default()
        };
        assert_eq!(
            eval_str("session_duration <= 5min", &q).unwrap(),
            Value::Bool(true)
        );
        q.duration = Duration::from_secs(301);
        assert_eq!(
            eval_str("session_duration <= 5min", &q).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn no_orphan_bracket_children_happy() {
        let mut q = MockQ::default();
        let mut parent = order("parent", OrderStatusName::Filled, 100.0);
        parent.parent_ref = None;
        let mut child = order("child", OrderStatusName::Cancelled, 0.0);
        child.parent_ref = Some("parent".into());
        q.orders = vec![parent, child];
        assert_eq!(
            eval_str("no_orphan_bracket_children", &q).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn no_orphan_bracket_children_detects_orphan() {
        let mut q = MockQ::default();
        let mut child = order("child", OrderStatusName::Cancelled, 0.0);
        child.parent_ref = Some("ghost".into());
        q.orders = vec![child];
        assert_eq!(
            eval_str("no_orphan_bracket_children", &q).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn unknown_identifier_error() {
        let q = MockQ::default();
        match eval_str("jiggle", &q) {
            Err(EvalError::UnknownIdent(n)) => assert_eq!(n, "jiggle"),
            other => panic!("expected UnknownIdent, got {other:?}"),
        }
    }

    #[test]
    fn unknown_function_error() {
        let q = MockQ::default();
        match eval_str("avg(1, 2)", &q) {
            Err(EvalError::UnknownFunction(n)) => assert_eq!(n, "avg"),
            other => panic!("expected UnknownFunction, got {other:?}"),
        }
    }

    #[test]
    fn order_out_of_range_error() {
        let q = MockQ::default();
        assert!(matches!(
            eval_str("orders[0].status == Filled", &q),
            Err(EvalError::PathResolve { .. })
        ));
    }

    #[test]
    fn bare_duration_literal() {
        let q = MockQ::default();
        // Durations parse to Num(seconds) at parse time (see parser.rs).
        assert_eq!(eval_str("30s", &q).unwrap(), Value::Num(30.0));
        assert_eq!(eval_str("5min", &q).unwrap(), Value::Num(300.0));
        assert_eq!(eval_str("1h", &q).unwrap(), Value::Num(3600.0));
        assert_eq!(eval_str("250ms", &q).unwrap(), Value::Num(0.25));
    }

    #[test]
    fn duration_parse_hh_mm_ss() {
        assert_eq!(
            parse_duration("00:05:00").unwrap(),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn duration_parse_units() {
        assert_eq!(parse_duration("5min").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn duration_parse_rejects_garbage() {
        assert!(parse_duration("nope").is_err());
        assert!(parse_duration("").is_err());
    }
}
