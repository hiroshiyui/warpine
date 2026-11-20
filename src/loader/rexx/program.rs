// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX instructions (ADR-0083): clauses turned into a flat list of
// instructions, with jump tables for the structured ones. A flat list lets
// SIGNAL and CALL go to any label, as REXX allows; IF / THEN / ELSE, DO /
// END and SELECT / WHEN / OTHERWISE / END are run through the tables.

use std::collections::HashMap;

use super::expr::{parse_expr, parse_expr_list, symbol_expr, Expr, ExprError};
use super::lexer::{Clause, Tok, Token};

/// Where a value is stored: a simple variable, a stem, or a compound
/// variable (its expression form names the stem and tail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Var(Vec<u8>),
    Compound(Expr),
}

/// A name in DROP or PROCEDURE EXPOSE: a symbol, or `(name)` for the names
/// listed in that variable's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRef {
    Name(Target),
    Indirect(Vec<u8>),
}

/// A DO instruction's repetition and conditions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoSpec {
    /// `DO var = start [TO end] [BY step] [FOR count]`.
    pub control: Option<(Target, Expr)>,
    pub to: Option<Expr>,
    pub by: Option<Expr>,
    /// FOR count, or `DO count` without a control variable.
    pub count: Option<Expr>,
    pub forever: bool,
    pub while_: Option<Expr>,
    pub until: Option<Expr>,
}

impl DoSpec {
    /// A plain DO group: runs its body once.
    pub fn is_simple(&self) -> bool {
        self.control.is_none() && self.count.is_none() && !self.forever && self.while_.is_none() && self.until.is_none()
    }
}

/// Where PARSE takes its string from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseSource {
    Arg,
    Pull,
    Var(Vec<u8>),
    Value(Option<Expr>),
    Source,
    Version,
    /// A line of the terminal's (the shell's) input.
    External,
}

/// A piece of a PARSE template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateItem {
    /// A variable taking a part of the string.
    Target(Target),
    /// `.`: a part of the string, dropped.
    Placeholder,
    /// A literal pattern, given as a string or in parentheses as a
    /// variable's value.
    Literal(PatternValue),
    /// An absolute position: `n`, `=n`, `=(v)`.
    Absolute(PatternValue),
    /// A relative position: `+n`, `-n`, `+(v)`, `-(v)`.
    Relative(bool, PatternValue),
}

/// A pattern's value: given in the program, or a variable's at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternValue {
    Const(Vec<u8>),
    Var(Expr),
}

/// One instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    Say(Option<Expr>),
    Assign(Target, Expr),
    /// An expression clause: a command for the current environment.
    Command(Expr),
    Nop,
    Label(Vec<u8>),
    If(Expr),
    Then,
    Else,
    Do(Box<DoSpec>),
    End(Option<Vec<u8>>),
    Select,
    When(Expr),
    Otherwise,
    Iterate(Option<Vec<u8>>),
    Leave(Option<Vec<u8>>),
    Call { name: Vec<u8>, literal: bool, args: Vec<Option<Expr>> },
    Signal(Vec<u8>),
    SignalValue(Expr),
    Return(Option<Expr>),
    Exit(Option<Expr>),
    Procedure(Vec<NameRef>),
    Drop(Vec<NameRef>),
    Interpret(Expr),
    Parse { upper: bool, source: ParseSource, templates: Vec<Vec<TemplateItem>> },
    Push(Option<Expr>),
    Queue(Option<Expr>),
    /// `ADDRESS env [command]`, `ADDRESS VALUE expr`, `ADDRESS` (swap).
    Address { env: Option<Vec<u8>>, command: Option<Expr>, value: Option<Expr> },
    NumericDigits(Option<Expr>),
    NumericFuzz(Option<Expr>),
    NumericForm(Option<Expr>),
    /// TRACE, accepted and not acted on in R1.
    Trace,
}

/// A program: its instructions, the line each comes from, its labels and
/// the jump tables.
#[derive(Debug, Default)]
pub struct Program {
    pub instrs: Vec<Instr>,
    pub lines: Vec<usize>,
    /// Label name → the label's instruction (the first of a name wins).
    pub labels: HashMap<Vec<u8>, usize>,
    /// For each IF: where its THEN instruction starts, where its ELSE
    /// instruction starts (if any), and the index after the whole IF.
    pub if_jumps: HashMap<usize, (usize, Option<usize>, usize)>,
    /// For each ELSE reached in sequence (its THEN part ran): where to go.
    pub else_skip: HashMap<usize, usize>,
    /// For each DO and SELECT: its END.
    pub end_of: HashMap<usize, usize>,
    /// For each END: its DO or SELECT.
    pub owner_of_end: HashMap<usize, usize>,
    /// For each WHEN: where its instruction starts, and where the next
    /// WHEN, OTHERWISE or END is.
    pub when_jumps: HashMap<usize, (usize, usize)>,
}

/// A syntax error: the REXX error number and the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxError {
    pub code: u32,
    pub line: usize,
}

const KEYWORDS: [&str; 27] = [
    "ADDRESS", "ARG", "CALL", "DO", "DROP", "ELSE", "END", "EXIT", "IF", "INTERPRET", "ITERATE", "LEAVE",
    "NOP", "NUMERIC", "OTHERWISE", "PARSE", "PROCEDURE", "PULL", "PUSH", "QUEUE", "RETURN", "SAY", "SELECT",
    "SIGNAL", "THEN", "TRACE", "WHEN",
];

fn upper_symbol(t: &Token) -> Option<Vec<u8>> {
    match &t.tok { Tok::Symbol(s) => Some(s.to_ascii_uppercase()), _ => None }
}

fn is_word(t: Option<&Token>, word: &str) -> bool {
    t.and_then(upper_symbol).is_some_and(|s| s == word.as_bytes())
}

/// Split clauses at labels and at THEN / ELSE / OTHERWISE, so that each of
/// those stands alone.
fn split_clauses(clauses: Vec<Clause>) -> Vec<Clause> {
    let mut out = Vec::new();
    let mut queue: std::collections::VecDeque<Clause> = clauses.into();
    while let Some(c) = queue.pop_front() {
        let t = &c.tokens;
        let first = t.first().and_then(upper_symbol);
        let assigning = matches!(t.get(1), Some(Token { tok: Tok::Op("="), .. }));
        // `name:` is a label of its own.
        if matches!(t.first().map(|t| &t.tok), Some(Tok::Symbol(_))) && matches!(t.get(1).map(|t| &t.tok), Some(Tok::Colon)) {
            out.push(Clause { line: c.line, tokens: t[..2].to_vec() });
            if t.len() > 2 { queue.push_front(Clause { line: c.line, tokens: t[2..].to_vec() }); }
            continue;
        }
        match first.as_deref() {
            Some(b"THEN" | b"ELSE" | b"OTHERWISE") if !assigning && t.len() > 1 => {
                out.push(Clause { line: c.line, tokens: t[..1].to_vec() });
                queue.push_front(Clause { line: c.line, tokens: t[1..].to_vec() });
            }
            Some(b"IF" | b"WHEN") if !assigning => {
                // The condition ends at THEN, outside parentheses.
                let mut depth = 0i32;
                let then = t.iter().enumerate().skip(1).find(|(_, tok)| {
                    match tok.tok { Tok::LParen => depth += 1, Tok::RParen => depth -= 1, _ => {} }
                    depth == 0 && is_word(Some(tok), "THEN")
                }).map(|(i, _)| i);
                match then {
                    Some(i) => {
                        out.push(Clause { line: c.line, tokens: t[..i].to_vec() });
                        queue.push_front(Clause { line: c.line, tokens: t[i..].to_vec() });
                    }
                    None => out.push(c),
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Parse a program's clauses into instructions and jump tables.
pub fn build(clauses: Vec<Clause>) -> Result<Program, SyntaxError> {
    let mut p = Program::default();
    for c in split_clauses(clauses) {
        let instr = parse_instr(&c.tokens).map_err(|code| SyntaxError { code, line: c.line })?;
        if let Instr::Label(name) = &instr { p.labels.entry(name.clone()).or_insert(p.instrs.len()); }
        p.instrs.push(instr);
        p.lines.push(c.line);
    }
    let mut i = 0;
    while i < p.instrs.len() { i = scan(&mut p, i)?; }
    Ok(p)
}

/// Fill the jump tables for the instruction at `i`; returns the index
/// after it (after a whole IF, DO … END or SELECT … END).
fn scan(p: &mut Program, i: usize) -> Result<usize, SyntaxError> {
    let err = |p: &Program, code: u32, at: usize| SyntaxError { code, line: p.lines.get(at).or(p.lines.last()).copied().unwrap_or(0) };
    let len = p.instrs.len();
    match &p.instrs[i] {
        Instr::If(_) => {
            if !matches!(p.instrs.get(i + 1), Some(Instr::Then)) { return Err(err(p, 18, i)); }
            let then_start = i + 2;
            if then_start >= len { return Err(err(p, 14, i)); }
            let then_end = instruction(p, then_start)?;
            if matches!(p.instrs.get(then_end), Some(Instr::Else)) {
                let else_start = then_end + 1;
                if else_start >= len { return Err(err(p, 14, then_end)); }
                let else_end = instruction(p, else_start)?;
                p.if_jumps.insert(i, (then_start, Some(else_start), else_end));
                p.else_skip.insert(then_end, else_end);
                Ok(else_end)
            } else {
                p.if_jumps.insert(i, (then_start, None, then_end));
                Ok(then_end)
            }
        }
        Instr::Do(_) => {
            let mut j = i + 1;
            while j < len && !matches!(p.instrs[j], Instr::End(_)) { j = scan(p, j)?; }
            if j >= len { return Err(err(p, 14, i)); }
            p.end_of.insert(i, j);
            p.owner_of_end.insert(j, i);
            Ok(j + 1)
        }
        Instr::Select => {
            let mut j = i + 1;
            if !matches!(p.instrs.get(j), Some(Instr::When(_))) { return Err(err(p, 7, i)); }
            while matches!(p.instrs.get(j), Some(Instr::When(_))) {
                if !matches!(p.instrs.get(j + 1), Some(Instr::Then)) { return Err(err(p, 18, j)); }
                if j + 2 >= len { return Err(err(p, 14, j)); }
                let next = instruction(p, j + 2)?;
                p.when_jumps.insert(j, (j + 2, next));
                j = next;
            }
            if matches!(p.instrs.get(j), Some(Instr::Otherwise)) {
                j += 1;
                while j < len && !matches!(p.instrs[j], Instr::End(_)) { j = scan(p, j)?; }
            }
            if !matches!(p.instrs.get(j), Some(Instr::End(_))) { return Err(err(p, if j >= len { 14 } else { 7 }, i)); }
            p.end_of.insert(i, j);
            p.owner_of_end.insert(j, i);
            Ok(j + 1)
        }
        // Each needs its IF, SELECT or DO.
        Instr::Then | Instr::Else => Err(err(p, 8, i)),
        Instr::When(_) | Instr::Otherwise => Err(err(p, 9, i)),
        Instr::End(_) => Err(err(p, 10, i)),
        _ => Ok(i + 1),
    }
}

/// The instruction starting at `i` as the subject of THEN / ELSE / WHEN:
/// one instruction (a label is none).
fn instruction(p: &mut Program, i: usize) -> Result<usize, SyntaxError> {
    if matches!(p.instrs[i], Instr::Label(_)) {
        return Err(SyntaxError { code: 20, line: p.lines[i] });
    }
    scan(p, i)
}

fn expr_of(tokens: &[Token]) -> Result<Expr, u32> {
    parse_expr(tokens).map_err(|ExprError(code)| code)
}

fn opt_expr(tokens: &[Token]) -> Result<Option<Expr>, u32> {
    if tokens.is_empty() { Ok(None) } else { expr_of(tokens).map(Some) }
}

/// A symbol as an assignment's target (Error 31 for a constant one).
pub fn target_of(sym: &[u8]) -> Result<Target, u32> {
    match symbol_expr(sym) {
        Expr::Var(name) => Ok(Target::Var(name)),
        c @ Expr::Compound { .. } => Ok(Target::Compound(c)),
        _ => Err(31),
    }
}

/// Parse one clause into an instruction (Err: the REXX error number).
fn parse_instr(t: &[Token]) -> Result<Instr, u32> {
    // A label.
    if let (Some(Tok::Symbol(name)), Some(Tok::Colon)) = (t.first().map(|x| &x.tok), t.get(1).map(|x| &x.tok)) {
        return Ok(Instr::Label(name.to_ascii_uppercase()));
    }
    if let Some(Token { tok: Tok::Colon, .. }) = t.first() { return Err(19); }
    // An assignment: a symbol followed by `=`.
    if let (Some(Tok::Symbol(name)), Some(Tok::Op("="))) = (t.first().map(|x| &x.tok), t.get(1).map(|x| &x.tok)) {
        let target = target_of(name)?;
        let value = if t.len() > 2 { expr_of(&t[2..])? } else { Expr::Lit(Vec::new()) };
        return Ok(Instr::Assign(target, value));
    }
    let keyword = t.first().and_then(upper_symbol).filter(|k| KEYWORDS.iter().any(|w| w.as_bytes() == k.as_slice()));
    let Some(keyword) = keyword else { return Ok(Instr::Command(expr_of(t)?)) };
    let rest = &t[1..];
    match keyword.as_slice() {
        b"SAY" => Ok(Instr::Say(opt_expr(rest)?)),
        b"NOP" => if rest.is_empty() { Ok(Instr::Nop) } else { Err(21) },
        b"IF" => Ok(Instr::If(expr_of(rest)?)),
        b"THEN" => if rest.is_empty() { Ok(Instr::Then) } else { Err(8) },
        b"ELSE" => if rest.is_empty() { Ok(Instr::Else) } else { Err(8) },
        b"SELECT" => if rest.is_empty() { Ok(Instr::Select) } else { Err(21) },
        b"WHEN" => Ok(Instr::When(expr_of(rest)?)),
        b"OTHERWISE" => if rest.is_empty() { Ok(Instr::Otherwise) } else { Err(21) },
        b"END" => single_name(rest).map(Instr::End),
        b"ITERATE" => single_name(rest).map(Instr::Iterate),
        b"LEAVE" => single_name(rest).map(Instr::Leave),
        b"DO" => parse_do(rest).map(|d| Instr::Do(Box::new(d))),
        b"EXIT" => Ok(Instr::Exit(opt_expr(rest)?)),
        b"RETURN" => Ok(Instr::Return(opt_expr(rest)?)),
        b"INTERPRET" => Ok(Instr::Interpret(expr_of(rest)?)),
        b"PUSH" => Ok(Instr::Push(opt_expr(rest)?)),
        b"QUEUE" => Ok(Instr::Queue(opt_expr(rest)?)),
        b"CALL" => {
            let (name, literal) = match rest.first().map(|x| &x.tok) {
                Some(Tok::Symbol(s)) => (s.to_ascii_uppercase(), false),
                Some(Tok::Str(s)) => (s.clone(), true),
                _ => return Err(19),
            };
            if !literal && matches!(name.as_slice(), b"ON" | b"OFF") { return Err(25); }
            let args = parse_expr_list(&rest[1..]).map_err(|ExprError(c)| c)?;
            Ok(Instr::Call { name, literal, args })
        }
        b"SIGNAL" => {
            match rest.first().map(|x| &x.tok) {
                Some(Tok::Symbol(s)) if s.eq_ignore_ascii_case(b"VALUE") => Ok(Instr::SignalValue(expr_of(&rest[1..])?)),
                Some(Tok::Symbol(s)) if s.eq_ignore_ascii_case(b"ON") || s.eq_ignore_ascii_case(b"OFF") => Err(25),
                Some(Tok::Symbol(s)) if rest.len() == 1 => Ok(Instr::Signal(s.to_ascii_uppercase())),
                Some(Tok::Str(s)) if rest.len() == 1 => Ok(Instr::Signal(s.clone())),
                Some(Tok::LParen) => Ok(Instr::SignalValue(expr_of(rest)?)),
                _ => Err(19),
            }
        }
        b"PROCEDURE" => {
            if rest.is_empty() { return Ok(Instr::Procedure(Vec::new())); }
            if !is_word(rest.first(), "EXPOSE") { return Err(25); }
            name_refs(&rest[1..]).map(Instr::Procedure)
        }
        b"DROP" => name_refs(rest).map(Instr::Drop),
        b"ADDRESS" => parse_address(rest),
        b"NUMERIC" => {
            let sub = rest.first().and_then(upper_symbol).ok_or(25u32)?;
            let value = opt_expr(&rest[1..])?;
            match sub.as_slice() {
                b"DIGITS" => Ok(Instr::NumericDigits(value)),
                b"FUZZ" => Ok(Instr::NumericFuzz(value)),
                b"FORM" => {
                    // FORM SCIENTIFIC | ENGINEERING | [VALUE] expr.
                    let form = match rest.get(1).and_then(upper_symbol).as_deref() {
                        Some(b"SCIENTIFIC") | Some(b"ENGINEERING") if rest.len() == 2 => Some(Expr::Lit(upper_symbol(&rest[1]).unwrap())),
                        Some(b"VALUE") => opt_expr(&rest[2..])?,
                        _ => value,
                    };
                    Ok(Instr::NumericForm(form))
                }
                _ => Err(25),
            }
        }
        b"PARSE" => parse_parse(rest),
        b"ARG" => Ok(Instr::Parse { upper: true, source: ParseSource::Arg, templates: templates(rest)? }),
        b"PULL" => Ok(Instr::Parse { upper: true, source: ParseSource::Pull, templates: templates(rest)? }),
        b"TRACE" => Ok(Instr::Trace),
        _ => Err(19),
    }
}

/// Nothing, or one symbol (END, ITERATE, LEAVE).
fn single_name(rest: &[Token]) -> Result<Option<Vec<u8>>, u32> {
    match rest {
        [] => Ok(None),
        [t] => upper_symbol(t).map(Some).ok_or(20),
        _ => Err(21),
    }
}

/// Symbols and `(name)` indirections.
fn name_refs(rest: &[Token]) -> Result<Vec<NameRef>, u32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match &rest[i].tok {
            Tok::Symbol(s) => { out.push(NameRef::Name(target_of(s)?)); i += 1; }
            Tok::LParen => {
                let (Some(Tok::Symbol(s)), Some(Tok::RParen)) = (rest.get(i + 1).map(|t| &t.tok), rest.get(i + 2).map(|t| &t.tok)) else { return Err(19) };
                out.push(NameRef::Indirect(s.to_ascii_uppercase()));
                i += 3;
            }
            _ => return Err(20),
        }
    }
    Ok(out)
}

fn parse_address(rest: &[Token]) -> Result<Instr, u32> {
    match rest.first().map(|t| &t.tok) {
        None => Ok(Instr::Address { env: None, command: None, value: None }),
        Some(Tok::Symbol(s)) if s.eq_ignore_ascii_case(b"VALUE") => {
            Ok(Instr::Address { env: None, command: None, value: Some(expr_of(&rest[1..])?) })
        }
        Some(Tok::LParen) => Ok(Instr::Address { env: None, command: None, value: Some(expr_of(rest)?) }),
        Some(Tok::Symbol(s)) | Some(Tok::Str(s)) => {
            let env = if matches!(rest[0].tok, Tok::Symbol(_)) { s.to_ascii_uppercase() } else { s.clone() };
            Ok(Instr::Address { env: Some(env), command: opt_expr(&rest[1..])?, value: None })
        }
        _ => Err(19),
    }
}

/// `DO [repetitor] [conditional]`.
fn parse_do(rest: &[Token]) -> Result<DoSpec, u32> {
    let mut spec = DoSpec::default();
    // Split at the keywords TO / BY / FOR / WHILE / UNTIL (outside parens).
    let mut parts: Vec<(Option<Vec<u8>>, Vec<Token>)> = vec![(None, Vec::new())];
    let mut depth = 0i32;
    let keywords: &[&[u8]] = &[b"TO", b"BY", b"FOR", b"WHILE", b"UNTIL"];
    for (k, tok) in rest.iter().enumerate() {
        match tok.tok { Tok::LParen => depth += 1, Tok::RParen => depth -= 1, _ => {} }
        let word = upper_symbol(tok).filter(|w| keywords.contains(&w.as_slice()));
        // `DO WHILE x` / `DO UNTIL x` / `DO FOREVER` start with their word.
        if depth == 0 && let Some(w) = word && !(k == 1 && matches!(rest[0].tok, Tok::Symbol(_)) && matches!(rest[1].tok, Tok::Op("="))) {
            parts.push((Some(w), Vec::new()));
            continue;
        }
        parts.last_mut().unwrap().1.push(tok.clone());
    }
    let (_, first) = parts.remove(0);
    if !first.is_empty() {
        let is_control = matches!((first.first().map(|t| &t.tok), first.get(1).map(|t| &t.tok)), (Some(Tok::Symbol(_)), Some(Tok::Op("="))));
        if is_control {
            let Tok::Symbol(name) = &first[0].tok else { unreachable!() };
            spec.control = Some((target_of(name)?, expr_of(&first[2..])?));
        } else if first.len() == 1 && is_word(first.first(), "FOREVER") {
            spec.forever = true;
        } else {
            spec.count = Some(expr_of(&first)?);
        }
    }
    for (word, tokens) in parts {
        let e = Some(expr_of(&tokens)?);
        let slot = match word.as_deref() {
            Some(b"TO") if spec.control.is_some() => &mut spec.to,
            Some(b"BY") if spec.control.is_some() => &mut spec.by,
            Some(b"FOR") if spec.control.is_some() => &mut spec.count,
            Some(b"WHILE") => &mut spec.while_,
            Some(b"UNTIL") => &mut spec.until,
            _ => return Err(27),
        };
        if slot.is_some() { return Err(27); }
        *slot = e;
    }
    if spec.while_.is_some() && spec.until.is_some() { return Err(27); }
    Ok(spec)
}

fn parse_parse(rest: &[Token]) -> Result<Instr, u32> {
    let mut i = 0;
    let upper = is_word(rest.first(), "UPPER");
    if upper { i += 1; }
    let sub = rest.get(i).and_then(upper_symbol).ok_or(25u32)?;
    i += 1;
    let (source, template_tokens) = match sub.as_slice() {
        b"ARG" => (ParseSource::Arg, &rest[i..]),
        b"PULL" => (ParseSource::Pull, &rest[i..]),
        b"SOURCE" => (ParseSource::Source, &rest[i..]),
        b"VERSION" => (ParseSource::Version, &rest[i..]),
        b"EXTERNAL" | b"LINEIN" => (ParseSource::External, &rest[i..]),
        b"VAR" => {
            let name = rest.get(i).and_then(upper_symbol).ok_or(19u32)?;
            (ParseSource::Var(name), &rest[i + 1..])
        }
        b"VALUE" => {
            let with = rest[i..].iter().position(|t| is_word(Some(t), "WITH")).ok_or(38u32)?;
            (ParseSource::Value(opt_expr(&rest[i..i + with])?), &rest[i + with + 1..])
        }
        _ => return Err(25),
    };
    Ok(Instr::Parse { upper, source, templates: templates(template_tokens)? })
}

/// Templates separated by commas.
fn templates(tokens: &[Token]) -> Result<Vec<Vec<TemplateItem>>, u32> {
    tokens.split(|t| t.tok == Tok::Comma).map(template).collect()
}

fn template(tokens: &[Token]) -> Result<Vec<TemplateItem>, u32> {
    let mut out = Vec::new();
    let mut i = 0;
    let var_pattern = |i: usize| -> Result<(Expr, usize), u32> {
        let (Some(Tok::LParen), Some(Tok::Symbol(s)), Some(Tok::RParen)) =
            (tokens.get(i).map(|t| &t.tok), tokens.get(i + 1).map(|t| &t.tok), tokens.get(i + 2).map(|t| &t.tok)) else { return Err(38) };
        Ok((symbol_expr(s), i + 3))
    };
    let number = |t: Option<&Token>| match t.map(|t| &t.tok) {
        Some(Tok::Symbol(s)) if s.iter().all(u8::is_ascii_digit) => Some(s.clone()),
        _ => None,
    };
    while i < tokens.len() {
        match &tokens[i].tok {
            Tok::Symbol(s) if s == b"." => { out.push(TemplateItem::Placeholder); i += 1; }
            Tok::Symbol(s) if s.iter().all(u8::is_ascii_digit) => {
                out.push(TemplateItem::Absolute(PatternValue::Const(s.clone())));
                i += 1;
            }
            Tok::Symbol(s) => { out.push(TemplateItem::Target(target_of(s)?)); i += 1; }
            Tok::Str(s) => { out.push(TemplateItem::Literal(PatternValue::Const(s.clone()))); i += 1; }
            Tok::LParen => {
                let (e, next) = var_pattern(i)?;
                out.push(TemplateItem::Literal(PatternValue::Var(e)));
                i = next;
            }
            Tok::Op(op @ ("=" | "+" | "-")) => {
                let value = if let Some(n) = number(tokens.get(i + 1)) {
                    i += 2;
                    PatternValue::Const(n)
                } else {
                    let (e, next) = var_pattern(i + 1)?;
                    i = next;
                    PatternValue::Var(e)
                };
                out.push(match *op {
                    "=" => TemplateItem::Absolute(value),
                    "+" => TemplateItem::Relative(false, value),
                    _ => TemplateItem::Relative(true, value),
                });
            }
            _ => return Err(38),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::lexer::tokenize;

    fn program(src: &str) -> Program { build(tokenize(src.as_bytes()).unwrap()).unwrap() }
    fn error(src: &str) -> u32 { build(tokenize(src.as_bytes()).unwrap()).unwrap_err().code }

    #[test]
    fn clauses_split_and_instructions_parse() {
        let p = program("start: if a = 1 then say 'one'; else say 'other'\nx = 2\nsay");
        assert!(matches!(p.instrs[0], Instr::Label(ref n) if n == b"START"));
        assert!(matches!(p.instrs[1], Instr::If(_)));
        assert_eq!(p.instrs[2], Instr::Then);
        assert!(matches!(p.instrs[3], Instr::Say(Some(_))));
        assert_eq!(p.instrs[4], Instr::Else);
        assert!(matches!(p.instrs[6], Instr::Assign(Target::Var(ref n), _) if n == b"X"));
        assert_eq!(p.instrs[7], Instr::Say(None));
        assert_eq!(p.if_jumps[&1], (3, Some(5), 6));
        assert_eq!(p.else_skip[&4], 6);
        assert_eq!(p.labels[b"START".as_slice()], 0);
        assert!(matches!(program("say = 3").instrs[0], Instr::Assign(..)), "a keyword with = assigns");
        assert!(matches!(program("'dir' arg").instrs[0], Instr::Command(_)));
    }

    #[test]
    fn structures_match() {
        let p = program("do i = 1 to 3 by 1 while i < 5\n say i\nend i\nselect\n when x then nop\n otherwise say 'o'\nend");
        let Instr::Do(spec) = &p.instrs[0] else { panic!() };
        assert!(spec.control.is_some() && spec.to.is_some() && spec.by.is_some() && spec.while_.is_some());
        assert_eq!(p.end_of[&0], 2);
        assert_eq!(p.instrs[2], Instr::End(Some(b"I".to_vec())));
        assert_eq!(p.end_of[&3], 9);
        assert_eq!(p.when_jumps[&4], (6, 7));
        let Instr::Do(spec) = &program("do 3; end").instrs[0] else { panic!() };
        assert!(spec.count.is_some() && spec.control.is_none());
        let Instr::Do(spec) = &program("do forever until x; end").instrs[0] else { panic!() };
        assert!(spec.forever && spec.until.is_some());
        assert!(matches!(&program("do; end").instrs[0], Instr::Do(s) if s.is_simple()));
        assert_eq!(program("if a then if b then say 1; else say 2").if_jumps[&0], (2, None, 7), "ELSE binds to the inner IF");
    }

    #[test]
    fn templates_parse() {
        let Instr::Parse { upper, source, templates } = program("parse upper value 'a b' with x 3 y '-' +2 . =(p) (q) z, w").instrs.remove(0) else { panic!() };
        assert!(upper);
        assert!(matches!(source, ParseSource::Value(Some(_))));
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].len(), 9);
        assert!(matches!(templates[0][1], TemplateItem::Absolute(PatternValue::Const(ref n)) if n == b"3"));
        assert!(matches!(templates[0][4], TemplateItem::Relative(false, PatternValue::Const(_))));
        assert_eq!(templates[0][5], TemplateItem::Placeholder);
        assert!(matches!(templates[0][7], TemplateItem::Literal(PatternValue::Var(_))));
        assert!(matches!(program("arg a b").instrs[0], Instr::Parse { upper: true, source: ParseSource::Arg, .. }));
    }

    #[test]
    fn syntax_errors() {
        assert_eq!(error("do\nsay 1"), 14, "DO without END");
        assert_eq!(error("end"), 10);
        assert_eq!(error("else say 1"), 8);
        assert_eq!(error("if a say 1"), 18, "IF without THEN");
        assert_eq!(error("select\nsay 1\nend"), 7);
        assert_eq!(error("when a then nop"), 9);
        assert_eq!(error("1abc = 2"), 31);
        assert_eq!(error("do i = 1 to 2 to 3; end"), 27);
        assert_eq!(error("signal on error"), 25, "conditions come in R2");
        assert_eq!(error("nop x"), 21);
    }
}
