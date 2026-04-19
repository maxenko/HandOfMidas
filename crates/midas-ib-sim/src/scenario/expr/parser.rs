//! Hand-rolled recursive-descent parser for the scenario expression DSL.
//!
//! The grammar is the one documented in [`super`]. No external PEG crate —
//! the whole surface is ~300 LOC incl. tests and stays in the crate so that
//! reviewers can read it front-to-back without dep hopping.
//!
//! Errors carry the byte offset into the source so the caller can render
//! `expr[20..]: unexpected ')'`-style diagnostics (Wave 2 wires this into
//! scenario-load error context).

use super::ast::{BinOp, Expr, Index, Segment, Value};

/// All parser diagnostics. Byte offsets are into the original source string.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected character `{ch}` at byte {pos}")]
    UnexpectedChar { pos: usize, ch: char },
    #[error("unexpected end of input — expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("unterminated string literal starting at byte {pos}")]
    UnterminatedString { pos: usize },
    #[error("invalid number `{text}` at byte {pos}")]
    InvalidNumber { pos: usize, text: String },
    #[error("expected `{expected}` at byte {pos}, found `{found}`")]
    Expected {
        pos: usize,
        expected: &'static str,
        found: String,
    },
    #[error("trailing input at byte {pos}: `{rest}`")]
    TrailingInput { pos: usize, rest: String },
}

/// Parse an expression string into an AST.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let mut p = Parser::new(src);
    let expr = p.parse_or()?;
    p.skip_whitespace();
    if p.pos < p.src.len() {
        let rest = p.src[p.pos..].to_string();
        return Err(ParseError::TrailingInput { pos: p.pos, rest });
    }
    Ok(expr)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, expected: &[u8]) -> bool {
        self.skip_whitespace();
        if self.bytes[self.pos..].starts_with(expected) {
            self.pos += expected.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &'static str, bytes: &[u8]) -> Result<(), ParseError> {
        if self.eat(bytes) {
            Ok(())
        } else {
            let found = self
                .peek()
                .map(|b| (b as char).to_string())
                .unwrap_or_default();
            Err(ParseError::Expected {
                pos: self.pos,
                expected,
                found,
            })
        }
    }

    // ----- grammar rules -----

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        loop {
            self.skip_whitespace();
            if self.eat(b"||") || self.eat_keyword("or") {
                let rhs = self.parse_and()?;
                lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_cmp()?;
        loop {
            self.skip_whitespace();
            if self.eat(b"&&") || self.eat_keyword("and") {
                let rhs = self.parse_cmp()?;
                lhs = Expr::And(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_term()?;
        self.skip_whitespace();
        let op = if self.eat(b"==") {
            BinOp::Eq
        } else if self.eat(b"!=") {
            BinOp::Neq
        } else if self.eat(b"<=") {
            BinOp::Le
        } else if self.eat(b">=") {
            BinOp::Ge
        } else if self.eat(b"<") {
            BinOp::Lt
        } else if self.eat(b">") {
            BinOp::Gt
        } else {
            return Ok(lhs);
        };
        let rhs = self.parse_term()?;
        Ok(Expr::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        let Some(b) = self.peek() else {
            return Err(ParseError::UnexpectedEof { expected: "term" });
        };
        match b {
            b'(' => {
                self.bump();
                let e = self.parse_or()?;
                self.expect(")", b")")?;
                Ok(e)
            }
            b'"' | b'\'' => {
                let s = self.parse_string()?;
                Ok(Expr::Literal(Value::Str(s)))
            }
            b'0'..=b'9' | b'-' | b'+' => {
                let n = self.parse_number()?;
                // Duration literal: `5min`, `30s`, `1500ms`, `1h`, `250ms`.
                // A trailing lowercase alpha run immediately after a number
                // with no intervening whitespace is a unit suffix.
                if matches!(self.peek(), Some(b'a'..=b'z')) {
                    let start = self.pos;
                    while matches!(self.peek(), Some(b'a'..=b'z')) {
                        self.pos += 1;
                    }
                    let suffix = &self.src[start..self.pos];
                    if is_duration_suffix(suffix) {
                        let num_secs = match &n {
                            Value::Int(i) => *i as f64,
                            Value::Num(f) => *f,
                            _ => 0.0,
                        };
                        let scaled = scale_by_suffix(num_secs, suffix);
                        return Ok(Expr::Literal(Value::Num(scaled)));
                    }
                    // Not a known suffix — rewind so the next rule can complain.
                    self.pos = start;
                }
                // Time-of-day literal: `00:00:05` / `HH:MM:SS` style. If the
                // parsed literal is an `Int` and the stream immediately has
                // a `:` followed by two more `:-separated` digit blocks,
                // fold into a Duration (seconds).
                if let Value::Int(h) = n {
                    if self.peek() == Some(b':') {
                        let save = self.pos;
                        self.bump();
                        if let Ok(Value::Int(m)) = self.parse_number() {
                            if self.peek() == Some(b':') {
                                self.bump();
                                if let Ok(Value::Int(sec)) = self.parse_number() {
                                    let total =
                                        (h as f64) * 3600.0 + (m as f64) * 60.0 + sec as f64;
                                    return Ok(Expr::Literal(Value::Num(total)));
                                }
                            }
                        }
                        // Rewind on parse failure.
                        self.pos = save;
                    }
                }
                Ok(Expr::Literal(n))
            }
            b if b.is_ascii_alphabetic() || b == b'_' => self.parse_ident_expr(),
            _ => Err(ParseError::UnexpectedChar {
                pos: self.pos,
                ch: b as char,
            }),
        }
    }

    fn parse_ident_expr(&mut self) -> Result<Expr, ParseError> {
        let ident = self.parse_ident()?;
        // Booleans are keywords.
        if ident == "true" {
            return Ok(Expr::Literal(Value::Bool(true)));
        }
        if ident == "false" {
            return Ok(Expr::Literal(Value::Bool(false)));
        }
        self.skip_whitespace();
        // Function call?
        if self.peek() == Some(b'(') {
            self.bump();
            let mut args = Vec::new();
            self.skip_whitespace();
            if self.peek() != Some(b')') {
                loop {
                    let arg = self.parse_or()?;
                    args.push(arg);
                    self.skip_whitespace();
                    if self.eat(b",") {
                        continue;
                    } else {
                        break;
                    }
                }
            }
            self.expect(")", b")")?;
            return Ok(Expr::Call { name: ident, args });
        }
        // Path?
        let mut segments = Vec::new();
        loop {
            self.skip_whitespace();
            if self.eat(b".") {
                let field = self.parse_ident()?;
                segments.push(Segment::Field(field));
            } else if self.peek() == Some(b'[') {
                self.bump();
                let idx = self.parse_index()?;
                self.expect("]", b"]")?;
                segments.push(Segment::Index(idx));
            } else {
                break;
            }
        }
        if segments.is_empty() {
            // Bare identifier — lookup falls to the query layer (status
            // names, built-in booleans, etc.). Represent as a path with no
            // segments so the interpreter has a uniform entry point.
            Ok(Expr::Path {
                root: ident,
                segments: Vec::new(),
            })
        } else {
            Ok(Expr::Path {
                root: ident,
                segments,
            })
        }
    }

    fn parse_index(&mut self) -> Result<Index, ParseError> {
        self.skip_whitespace();
        let Some(b) = self.peek() else {
            return Err(ParseError::UnexpectedEof { expected: "index" });
        };
        match b {
            b'*' => {
                self.bump();
                Ok(Index::Wildcard)
            }
            b'"' | b'\'' => {
                let s = self.parse_string()?;
                Ok(Index::Str(s))
            }
            b'-' | b'0'..=b'9' => {
                let n = self.parse_number()?;
                match n {
                    Value::Int(i) => Ok(Index::Int(i)),
                    Value::Num(f) if f.fract() == 0.0 => Ok(Index::Int(f as i64)),
                    _ => Err(ParseError::InvalidNumber {
                        pos: self.pos,
                        text: "non-integer index".into(),
                    }),
                }
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                let ident = self.parse_ident_with_hyphen()?;
                Ok(Index::Ident(ident))
            }
            _ => Err(ParseError::UnexpectedChar {
                pos: self.pos,
                ch: b as char,
            }),
        }
    }

    fn parse_ident(&mut self) -> Result<String, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            let ch = self.peek().map(|b| b as char).unwrap_or('\0');
            return Err(ParseError::UnexpectedChar { pos: start, ch });
        }
        Ok(self.src[start..self.pos].to_string())
    }

    /// Identifier that may contain internal hyphens — used inside `[...]`
    /// so `orders[smoke-1]` parses as a single key. We don't want hyphens
    /// in general idents (would clash with subtraction if we ever add it).
    fn parse_ident_with_hyphen(&mut self) -> Result<String, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            let ch = self.peek().map(|b| b as char).unwrap_or('\0');
            return Err(ParseError::UnexpectedChar { pos: start, ch });
        }
        Ok(self.src[start..self.pos].to_string())
    }

    /// Match a keyword like `and` / `or` only when followed by non-ident.
    fn eat_keyword(&mut self, kw: &str) -> bool {
        self.skip_whitespace();
        let saved = self.pos;
        if !self.bytes[self.pos..].starts_with(kw.as_bytes()) {
            return false;
        }
        let after = self.pos + kw.len();
        // Must not be followed by an ident-continuation byte.
        if let Some(&b) = self.bytes.get(after) {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos = saved;
                return false;
            }
        }
        self.pos = after;
        true
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        let quote = self.bump().expect("caller ensured quote");
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if b == quote {
                self.bump();
                return Ok(out);
            }
            if b == b'\\' {
                self.bump();
                match self.bump() {
                    Some(b'\\') => out.push('\\'),
                    Some(b'"') => out.push('"'),
                    Some(b'\'') => out.push('\''),
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(other) => out.push(other as char),
                    None => return Err(ParseError::UnterminatedString { pos: start }),
                }
            } else {
                out.push(b as char);
                self.bump();
            }
        }
        Err(ParseError::UnterminatedString { pos: start })
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        if let Some(b'-' | b'+') = self.peek() {
            self.bump();
        }
        let mut saw_digit = false;
        let mut saw_dot = false;
        while let Some(b) = self.peek() {
            match b {
                b'0'..=b'9' => {
                    saw_digit = true;
                    self.bump();
                }
                b'.' if !saw_dot => {
                    // Peek ahead: only treat as decimal if followed by a digit.
                    if matches!(self.peek_at(1), Some(b'0'..=b'9')) {
                        saw_dot = true;
                        self.bump();
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        if !saw_digit {
            return Err(ParseError::InvalidNumber {
                pos: start,
                text: self.src[start..self.pos].to_string(),
            });
        }
        let text = &self.src[start..self.pos];
        if saw_dot {
            text.parse::<f64>()
                .map(Value::Num)
                .map_err(|_| ParseError::InvalidNumber {
                    pos: start,
                    text: text.into(),
                })
        } else {
            text.parse::<i64>()
                .map(Value::Int)
                .map_err(|_| ParseError::InvalidNumber {
                    pos: start,
                    text: text.into(),
                })
        }
    }
}

fn is_duration_suffix(s: &str) -> bool {
    matches!(
        s,
        "ms" | "s" | "sec" | "secs" | "m" | "min" | "h" | "hr" | "hour" | "hours"
    )
}

fn scale_by_suffix(n: f64, suffix: &str) -> f64 {
    match suffix {
        "ms" => n / 1000.0,
        "s" | "sec" | "secs" => n,
        "m" | "min" => n * 60.0,
        "h" | "hr" | "hour" | "hours" => n * 3600.0,
        _ => n,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lit_num(n: f64) -> Expr {
        Expr::Literal(Value::Num(n))
    }

    fn lit_int(i: i64) -> Expr {
        Expr::Literal(Value::Int(i))
    }

    fn ident(name: &str) -> Expr {
        Expr::Path {
            root: name.into(),
            segments: vec![],
        }
    }

    #[test]
    fn literal_integer() {
        assert_eq!(parse("42").unwrap(), lit_int(42));
    }

    #[test]
    fn literal_float() {
        assert_eq!(parse("2.5").unwrap(), lit_num(2.5));
    }

    #[test]
    fn literal_negative_float() {
        assert_eq!(parse("-1.5").unwrap(), lit_num(-1.5));
    }

    #[test]
    fn literal_string_double() {
        assert_eq!(
            parse("\"hi\"").unwrap(),
            Expr::Literal(Value::Str("hi".into()))
        );
    }

    #[test]
    fn literal_string_single() {
        assert_eq!(
            parse("'hi'").unwrap(),
            Expr::Literal(Value::Str("hi".into()))
        );
    }

    #[test]
    fn literal_bool_true_false() {
        assert_eq!(parse("true").unwrap(), Expr::Literal(Value::Bool(true)));
        assert_eq!(parse("false").unwrap(), Expr::Literal(Value::Bool(false)));
    }

    #[test]
    fn bare_identifier() {
        assert_eq!(parse("Filled").unwrap(), ident("Filled"));
    }

    #[test]
    fn dotted_path() {
        let e = parse("orders.total").unwrap();
        assert_eq!(
            e,
            Expr::Path {
                root: "orders".into(),
                segments: vec![Segment::Field("total".into())],
            }
        );
    }

    #[test]
    fn indexed_path_integer() {
        let e = parse("orders[0].status").unwrap();
        assert_eq!(
            e,
            Expr::Path {
                root: "orders".into(),
                segments: vec![
                    Segment::Index(Index::Int(0)),
                    Segment::Field("status".into())
                ],
            }
        );
    }

    #[test]
    fn indexed_path_string() {
        let e = parse("orders[\"smoke-1\"].status").unwrap();
        let Expr::Path { segments, .. } = &e else {
            panic!("expected Path, got {e:?}");
        };
        assert!(matches!(segments[0], Segment::Index(Index::Str(_))));
    }

    #[test]
    fn indexed_path_ident_with_hyphen() {
        // Bare (unquoted) order_ref inside brackets — common in fixtures.
        let e = parse("orders[smoke-1].status").unwrap();
        let Expr::Path { segments, .. } = &e else {
            panic!("expected Path, got {e:?}");
        };
        match &segments[0] {
            Segment::Index(Index::Ident(s)) => assert_eq!(s, "smoke-1"),
            other => panic!("expected Index::Ident, got {other:?}"),
        }
    }

    #[test]
    fn indexed_path_wildcard() {
        let e = parse("orders[*].filled_qty").unwrap();
        let Expr::Path { segments, .. } = &e else {
            panic!("expected Path");
        };
        assert!(matches!(segments[0], Segment::Index(Index::Wildcard)));
    }

    #[test]
    fn positions_by_symbol() {
        let e = parse("positions[AAPL].quantity > 0").unwrap();
        assert!(matches!(e, Expr::BinOp { op: BinOp::Gt, .. }));
    }

    #[test]
    fn function_call_no_args() {
        let e = parse("count()").unwrap();
        assert_eq!(
            e,
            Expr::Call {
                name: "count".into(),
                args: vec![],
            }
        );
    }

    #[test]
    fn function_call_one_arg() {
        let e = parse("sum(orders[*].filled_qty)").unwrap();
        let Expr::Call { name, args } = e else {
            panic!("expected Call");
        };
        assert_eq!(name, "sum");
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn function_call_many_args() {
        let e = parse("max(1, 2, 3)").unwrap();
        let Expr::Call { name, args } = e else {
            panic!("expected Call");
        };
        assert_eq!(name, "max");
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn comparison_eq() {
        let e = parse("orders[0].status == Filled").unwrap();
        assert!(matches!(e, Expr::BinOp { op: BinOp::Eq, .. }));
    }

    #[test]
    fn comparison_neq() {
        let e = parse("x != 5").unwrap();
        assert!(matches!(e, Expr::BinOp { op: BinOp::Neq, .. }));
    }

    #[test]
    fn comparison_all_ops() {
        for (src, op) in [
            ("a < b", BinOp::Lt),
            ("a <= b", BinOp::Le),
            ("a > b", BinOp::Gt),
            ("a >= b", BinOp::Ge),
        ] {
            let e = parse(src).unwrap();
            match e {
                Expr::BinOp { op: got, .. } => assert_eq!(got, op, "src={src}"),
                _ => panic!("expected binop for {src}"),
            }
        }
    }

    #[test]
    fn logical_and_symbolic() {
        let e = parse("a == 1 && b == 2").unwrap();
        assert!(matches!(e, Expr::And(..)));
    }

    #[test]
    fn logical_and_word() {
        let e = parse("a == 1 and b == 2").unwrap();
        assert!(matches!(e, Expr::And(..)));
    }

    #[test]
    fn logical_or_symbolic() {
        let e = parse("a == 1 || b == 2").unwrap();
        assert!(matches!(e, Expr::Or(..)));
    }

    #[test]
    fn logical_or_word() {
        let e = parse("a == 1 or b == 2").unwrap();
        assert!(matches!(e, Expr::Or(..)));
    }

    #[test]
    fn and_precedence_over_or() {
        // `a || b && c` == `a || (b && c)`
        let e = parse("a || b && c").unwrap();
        match e {
            Expr::Or(_, rhs) => assert!(matches!(*rhs, Expr::And(..))),
            _ => panic!("expected top-level Or"),
        }
    }

    #[test]
    fn parens_override_precedence() {
        // `(a || b) && c`
        let e = parse("(a || b) && c").unwrap();
        match e {
            Expr::And(lhs, _) => assert!(matches!(*lhs, Expr::Or(..))),
            _ => panic!("expected top-level And"),
        }
    }

    #[test]
    fn whitespace_is_flexible() {
        let e = parse("  orders [ 0 ] . status  ==  Filled  ").unwrap();
        assert!(matches!(e, Expr::BinOp { op: BinOp::Eq, .. }));
    }

    #[test]
    fn keyword_vs_identifier_boundary() {
        // "order" should NOT be parsed as "or" keyword — the identifier continues.
        let e = parse("order == 1").unwrap();
        match e {
            Expr::BinOp { lhs, .. } => assert_eq!(*lhs, ident("order")),
            _ => panic!("expected BinOp"),
        }
    }

    #[test]
    fn session_nested_access() {
        let e = parse("session[0].msg_count").unwrap();
        let Expr::Path { root, segments } = e else {
            panic!("expected Path");
        };
        assert_eq!(root, "session");
        assert_eq!(segments.len(), 2);
    }

    // ---- malformed inputs ----

    #[test]
    fn rejects_unterminated_string() {
        assert!(matches!(
            parse("\"hi").unwrap_err(),
            ParseError::UnterminatedString { .. }
        ));
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(
            parse("").unwrap_err(),
            ParseError::UnexpectedEof { .. }
        ));
    }

    #[test]
    fn rejects_only_whitespace() {
        assert!(matches!(
            parse("   ").unwrap_err(),
            ParseError::UnexpectedEof { .. }
        ));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(matches!(
            parse("1 + 2 + @").unwrap_err(),
            ParseError::TrailingInput { .. }
                | ParseError::Expected { .. }
                | ParseError::UnexpectedChar { .. }
        ));
    }

    #[test]
    fn rejects_unclosed_paren() {
        assert!(matches!(
            parse("(1 == 2").unwrap_err(),
            ParseError::Expected { expected: ")", .. }
        ));
    }

    #[test]
    fn rejects_unclosed_bracket() {
        assert!(matches!(
            parse("orders[0").unwrap_err(),
            ParseError::Expected { expected: "]", .. }
        ));
    }

    #[test]
    fn rejects_bare_operator() {
        assert!(parse("<").is_err());
    }

    #[test]
    fn rejects_dangling_dot() {
        assert!(parse("orders.").is_err());
    }

    #[test]
    fn rejects_empty_bracket() {
        assert!(parse("orders[]").is_err());
    }
}
