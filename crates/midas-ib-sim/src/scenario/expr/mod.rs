//! Scenario expression language — parser, AST, interpreter, and query trait.
//!
//! Grammar (hand-rolled recursive-descent; no external parser crate):
//!
//! ```text
//! expr       = or_expr
//! or_expr    = and_expr (("||" | "or") and_expr)*
//! and_expr   = cmp_expr (("&&" | "and") cmp_expr)*
//! cmp_expr   = term (("==" | "!=" | "<" | "<=" | ">" | ">=") term)?
//! term       = literal | path | func_call | "(" expr ")"
//! path       = ident ("." ident | "[" (ident | string | integer) "]")*
//! func_call  = ident "(" (arg ("," arg)*)? ")"
//! arg        = expr
//! literal    = number | string | "true" | "false" | bare_symbol
//! ```
//!
//! Domain binding (see [`ScenarioQuery`]):
//! - `orders[idx]` / `orders[order_ref]` / `orders[*]` — order snapshots
//! - `positions[symbol]` — position snapshot
//! - `session[id]` — session metrics
//! - bare identifiers `Filled`, `Cancelled`, … — order-status symbols
//! - built-in booleans `all_orders_have_terminal_status`, `no_orphan_bracket_children`
//! - built-in values `session_duration`
//!
//! Closed function list: `sum`, `count`, `max`, `min`, `last_5s`.
//!
//! The interpreter is deliberately minimal — no variables, no loops, no
//! user-defined functions. See `plan/ib-sim/06-failure-injection.md` §"Expression language".

pub mod ast;
pub mod interpreter;
pub mod parser;
pub mod query;

pub use self::ast::{BinOp, Expr, Index, Value};
pub use self::interpreter::{eval, EvalError};
pub use self::parser::{parse, ParseError};
pub use self::query::{
    OrderSnapshot, OrderStatusName, PositionSnapshot, ScenarioQuery, SessionMetrics,
};
