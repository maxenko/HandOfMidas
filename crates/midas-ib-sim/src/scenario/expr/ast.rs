//! Parsed-expression AST + runtime values.

use std::fmt;

/// Parsed expression. Immutable and cheap to clone — scenarios hold a parsed
/// `Expr` on every `when:` and `assert`.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// Literal numeric (f64), string, boolean, or identifier-as-symbol.
    Literal(Value),
    /// Dotted / indexed path rooted at an identifier.
    Path {
        root: String,
        segments: Vec<Segment>,
    },
    /// Function call — closed list enforced at eval time.
    Call { name: String, args: Vec<Expr> },
    /// Binary operation.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Logical AND.
    And(Box<Expr>, Box<Expr>),
    /// Logical OR.
    Or(Box<Expr>, Box<Expr>),
}

/// Path navigation step — dot-access or bracket-index.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// `.field`
    Field(String),
    /// `[idx]` where `idx` is an integer literal, string, or bare ident.
    Index(Index),
}

/// Index payload within `[...]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Index {
    /// Numeric index — `orders[0]`.
    Int(i64),
    /// String-keyed index — `orders["smoke-1"]`.
    Str(String),
    /// Identifier used as a key — `positions[AAPL]`.
    Ident(String),
    /// `*` — wildcard over a collection (used with aggregate funcs).
    Wildcard,
}

/// Comparison + arithmetic binary operators.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BinOp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinOp::Eq => "==",
            BinOp::Neq => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        })
    }
}

/// Runtime value — the interpreter's universe.
#[derive(Clone, Debug)]
pub enum Value {
    /// Numeric literal / field value.
    Num(f64),
    /// Integer — only produced by `count(…)` and literal integer parsing.
    Int(i64),
    /// String literal / field value.
    Str(String),
    /// Boolean.
    Bool(bool),
    /// Ordered list — produced by `path[*]` expansions. The list carries
    /// `Value`s of one kind (aggregate funcs enforce their own type checks).
    List(Vec<Value>),
    /// Empty / missing — only emitted as the result of evaluating an absent
    /// optional field (never appears as an input literal).
    Null,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Num(a), Value::Int(b)) | (Value::Int(b), Value::Num(a)) => *a == *b as f64,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Null, Value::Null) => true,
            _ => false,
        }
    }
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Int(i) => Some(*i as f64),
            Value::Bool(true) => Some(1.0),
            Value::Bool(false) => Some(0.0),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Crude total ordering for comparison ops. Mixed-type compares are
    /// coerced numerically when possible, otherwise return `None`.
    pub fn cmp_total(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if let (Some(a), Some(b)) = (self.as_f64(), other.as_f64()) {
            // Treat NaN as unordered — callers propagate as eval error.
            return a.partial_cmp(&b);
        }
        if let (Value::Str(a), Value::Str(b)) = (self, other) {
            return Some(a.cmp(b));
        }
        None
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Num(_) => "number",
            Value::Int(_) => "int",
            Value::Str(_) => "string",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Null => "null",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(n) => write!(f, "{n}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Null => write!(f, "null"),
        }
    }
}
