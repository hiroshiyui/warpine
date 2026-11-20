// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX expressions (ADR-0083): parsed from a clause's tokens into a tree.
//
// Precedence, from loosest to tightest:
//   | &&                      or, exclusive or
//   &                         and
//   = \= <> >< > < >= <= \> \< == \== >> << >>= <<= \>> \<<   comparisons
//   || (blank) (abuttal)      concatenation
//   + -                       addition
//   * / % //                  multiplication
//   **                        power
//   + - \                     prefix
// Operators of one level go left to right (`2**3**2` is 64).

use super::lexer::{Tok, Token, is_symbol_char};

/// The most expressions and calls nested inside one another in one clause.
pub const MAX_EXPR_DEPTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, IntDiv, Rem, Pow,
    /// `||` and abuttal: joined with nothing between.
    Concat,
    /// Joined with one blank between.
    BlankConcat,
    Eq, Ne, Gt, Lt, Ge, Le,
    StrictEq, StrictNe, StrictGt, StrictLt, StrictGe, StrictLe,
    And, Or, Xor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp { Plus, Minus, Not }

/// A part of a compound symbol's tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailPart {
    /// A constant part (empty, or starting with a digit), as written in
    /// upper case.
    Const(Vec<u8>),
    /// A simple symbol: its value, or its name when it has none.
    Var(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A literal string or a constant symbol's value.
    Lit(Vec<u8>),
    /// A simple variable or a stem (`A.`), by its upper-case name.
    Var(Vec<u8>),
    /// A compound variable: its stem (`A.`) and tail.
    Compound { stem: Vec<u8>, tail: Vec<TailPart> },
    /// A function call: its name (upper case, unless given as a string),
    /// whether it was a string, and its arguments (some may be omitted).
    Call { name: Vec<u8>, literal: bool, args: Vec<Option<Expr>> },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

/// An expression syntax error: the REXX error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprError(pub u32);

/// Error 35: invalid expression.
pub const ERR_INVALID_EXPR: ExprError = ExprError(35);
/// Error 36: unmatched `(`.
const ERR_UNMATCHED_PAREN: ExprError = ExprError(36);
/// Error 37: unexpected `,` or `)`.
const ERR_UNEXPECTED_COMMA: ExprError = ExprError(37);
/// Error 11: too much nesting (a resource limit of Warpine's).
pub const ERR_TOO_DEEP: ExprError = ExprError(11);

/// What a symbol stands for in an expression.
pub fn symbol_expr(sym: &[u8]) -> Expr {
    let upper = sym.to_ascii_uppercase();
    if upper[0].is_ascii_digit() || upper[0] == b'.' {
        return Expr::Lit(upper);
    }
    match upper.iter().position(|&c| c == b'.') {
        Some(dot) if dot + 1 < upper.len() => {
            let stem = upper[..=dot].to_vec();
            let tail = upper[dot + 1..].split(|&c| c == b'.').map(|part| {
                if part.is_empty() || part[0].is_ascii_digit() { TailPart::Const(part.to_vec()) } else { TailPart::Var(part.to_vec()) }
            }).collect();
            Expr::Compound { stem, tail }
        }
        _ => Expr::Var(upper),
    }
}

/// Parse all of `tokens` as one expression.
pub fn parse_expr(tokens: &[Token]) -> Result<Expr, ExprError> {
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let e = p.expr(0)?;
    match p.peek() {
        None => Ok(e),
        Some(Tok::RParen) | Some(Tok::Comma) => Err(ERR_UNEXPECTED_COMMA),
        Some(_) => Err(ERR_INVALID_EXPR),
    }
}

/// Parse `tokens` as expressions separated by commas (CALL's arguments);
/// an empty one is omitted.
pub fn parse_expr_list(tokens: &[Token]) -> Result<Vec<Option<Expr>>, ExprError> {
    if tokens.is_empty() { return Ok(Vec::new()); }
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let mut out = Vec::new();
    loop {
        if matches!(p.peek(), Some(Tok::Comma) | None) { out.push(None); } else { out.push(Some(p.expr(0)?)); }
        match p.peek() {
            None => return Ok(out),
            Some(Tok::Comma) => p.pos += 1,
            Some(Tok::RParen) => return Err(ERR_UNEXPECTED_COMMA),
            Some(_) => return Err(ERR_INVALID_EXPR),
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
}

/// Binding power of a binary operator token (left), or `None`.
fn binary_op(tok: &Tok) -> Option<(BinOp, u8)> {
    let Tok::Op(op) = tok else { return None };
    Some(match *op {
        "|" => (BinOp::Or, 1), "&&" => (BinOp::Xor, 1),
        "&" => (BinOp::And, 2),
        "=" => (BinOp::Eq, 3), "\\=" | "<>" | "><" => (BinOp::Ne, 3), ">" => (BinOp::Gt, 3), "<" => (BinOp::Lt, 3),
        ">=" | "\\<" => (BinOp::Ge, 3), "<=" | "\\>" => (BinOp::Le, 3),
        "==" => (BinOp::StrictEq, 3), "\\==" => (BinOp::StrictNe, 3), ">>" => (BinOp::StrictGt, 3),
        "<<" => (BinOp::StrictLt, 3), ">>=" | "\\<<" => (BinOp::StrictGe, 3), "<<=" | "\\>>" => (BinOp::StrictLe, 3),
        "||" => (BinOp::Concat, 4),
        "+" => (BinOp::Add, 5), "-" => (BinOp::Sub, 5),
        "*" => (BinOp::Mul, 6), "/" => (BinOp::Div, 6), "%" => (BinOp::IntDiv, 6), "//" => (BinOp::Rem, 6),
        "**" => (BinOp::Pow, 7),
        _ => return None,
    })
}

/// The binding power of concatenation by blank or abuttal.
const CONCAT_POWER: u8 = 4;

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> { self.tokens.get(self.pos).map(|t| &t.tok) }

    fn expr(&mut self, min: u8) -> Result<Expr, ExprError> {
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH { return Err(ERR_TOO_DEEP); }
        let mut left = self.term()?;
        while let Some(token) = self.tokens.get(self.pos) {
            let (op, power) = match binary_op(&token.tok) {
                Some(b) => b,
                // A term right after a term: concatenation.
                None if matches!(token.tok, Tok::Symbol(_) | Tok::Str(_) | Tok::LParen) => {
                    (if token.space_before { BinOp::BlankConcat } else { BinOp::Concat }, CONCAT_POWER)
                }
                None => break,
            };
            if power <= min { break; }
            if binary_op(&token.tok).is_some() { self.pos += 1; }
            let right = self.expr(power)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    /// A term: a prefix operator and its term, a parenthesised
    /// expression, a function call, a string or a symbol.
    fn term(&mut self) -> Result<Expr, ExprError> {
        let Some(token) = self.tokens.get(self.pos) else { return Err(ERR_INVALID_EXPR) };
        self.pos += 1;
        match &token.tok {
            Tok::Op(op) if matches!(*op, "+" | "-" | "\\") => {
                let un = match *op { "+" => UnOp::Plus, "-" => UnOp::Minus, _ => UnOp::Not };
                self.depth += 1;
                if self.depth > MAX_EXPR_DEPTH { return Err(ERR_TOO_DEEP); }
                let e = self.term()?;
                self.depth -= 1;
                Ok(Expr::Unary(un, Box::new(e)))
            }
            Tok::LParen => {
                let e = self.expr(0)?;
                match self.peek() {
                    Some(Tok::RParen) => { self.pos += 1; Ok(e) }
                    Some(Tok::Comma) => Err(ERR_UNEXPECTED_COMMA),
                    _ => Err(ERR_UNMATCHED_PAREN),
                }
            }
            Tok::Str(s) | Tok::Symbol(s) => {
                let is_call = self.tokens.get(self.pos).is_some_and(|t| t.tok == Tok::LParen && !t.space_before);
                if is_call {
                    self.pos += 1;
                    let args = self.args()?;
                    let literal = matches!(token.tok, Tok::Str(_));
                    let name = if literal { s.clone() } else { s.to_ascii_uppercase() };
                    return Ok(Expr::Call { name, literal, args });
                }
                Ok(match &token.tok {
                    Tok::Str(s) => Expr::Lit(s.clone()),
                    _ => symbol_expr(s),
                })
            }
            Tok::RParen | Tok::Comma => Err(ERR_UNEXPECTED_COMMA),
            _ => Err(ERR_INVALID_EXPR),
        }
    }

    /// A call's arguments, after its `(`, up to and including its `)`.
    fn args(&mut self) -> Result<Vec<Option<Expr>>, ExprError> {
        let mut args = Vec::new();
        if self.peek() == Some(&Tok::RParen) { self.pos += 1; return Ok(args); }
        loop {
            if matches!(self.peek(), Some(Tok::Comma) | Some(Tok::RParen)) {
                args.push(None);
            } else {
                args.push(Some(self.expr(0)?));
            }
            match self.peek() {
                Some(Tok::Comma) => self.pos += 1,
                Some(Tok::RParen) => { self.pos += 1; return Ok(args); }
                _ => return Err(ERR_UNMATCHED_PAREN),
            }
        }
    }
}

/// Whether `name` is a valid symbol (for VALUE, SYMBOL and indirect names).
pub fn is_valid_symbol(name: &[u8]) -> bool {
    !name.is_empty() && name.iter().all(|&c| is_symbol_char(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::lexer::tokenize;

    fn parse(src: &str) -> Expr {
        let c = tokenize(src.as_bytes()).unwrap();
        parse_expr(&c[0].tokens).unwrap()
    }
    fn lit(s: &str) -> Box<Expr> { Box::new(Expr::Lit(s.as_bytes().to_vec())) }
    fn var(s: &str) -> Box<Expr> { Box::new(Expr::Var(s.as_bytes().to_vec())) }
    fn bin(op: BinOp, a: Box<Expr>, b: Box<Expr>) -> Box<Expr> { Box::new(Expr::Binary(op, a, b)) }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(parse("1+2*3"), *bin(BinOp::Add, lit("1"), bin(BinOp::Mul, lit("2"), lit("3"))));
        assert_eq!(parse("2**3**2"), *bin(BinOp::Pow, bin(BinOp::Pow, lit("2"), lit("3")), lit("2")), "left to right");
        assert_eq!(parse("-2**2"), *bin(BinOp::Pow, Box::new(Expr::Unary(UnOp::Minus, lit("2"))), lit("2")), "prefix binds tighter");
        assert_eq!(parse("a b||c"), *bin(BinOp::Concat, bin(BinOp::BlankConcat, var("A"), var("B")), var("C")));
        assert_eq!(parse("a = 1 & b"), *bin(BinOp::And, bin(BinOp::Eq, var("A"), lit("1")), var("B")));
        assert_eq!(parse("x'4142'x"), *bin(BinOp::Concat, var("X"), lit("AB")), "abuttal");
        assert_eq!(parse("(1 + 2) * 3"), *bin(BinOp::Mul, bin(BinOp::Add, lit("1"), lit("2")), lit("3")));
    }

    #[test]
    fn symbols_and_calls() {
        assert_eq!(parse("1e3"), Expr::Lit(b"1E3".to_vec()));
        assert_eq!(parse("Stem."), Expr::Var(b"STEM.".to_vec()));
        assert_eq!(parse("a.i.2"), Expr::Compound { stem: b"A.".to_vec(), tail: vec![TailPart::Var(b"I".to_vec()), TailPart::Const(b"2".to_vec())] });
        assert_eq!(parse("substr(s, 2,, 1)"), Expr::Call { name: b"SUBSTR".to_vec(), literal: false,
            args: vec![Some(*var("S")), Some(*lit("2")), None, Some(*lit("1"))] });
        assert_eq!(parse("'f'()"), Expr::Call { name: b"f".to_vec(), literal: true, args: vec![] });
        assert_eq!(parse("f (x)"), *bin(BinOp::BlankConcat, var("F"), var("X")), "a blank: no call");
    }

    #[test]
    fn errors() {
        let err = |s: &str| parse_expr(&tokenize(s.as_bytes()).unwrap()[0].tokens).unwrap_err().0;
        assert_eq!(err("(1 + 2"), 36);
        assert_eq!(err("1 + )"), 37);
        assert_eq!(err("1 +"), 35);
        assert_eq!(err("f(1"), 36);
        let deep = format!("{}1{}", "(".repeat(300), ")".repeat(300));
        assert_eq!(err(&deep), 11);
    }
}
