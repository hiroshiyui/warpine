// SPDX-License-Identifier: GPL-3.0-or-later
//
// The C preprocessor and tokenizer of Warpine's SDK tools: `os2rc`
// (ADR-0099) and `somidl` (ADR-0122) include it with `#[path]`.
//
// Logical lines (comments removed, continuations joined), `#define` with
// parameters, `#undef`, `#include "file"` / `<file>` on a search path,
// `#if` / `#ifdef` / `#ifndef` / `#elif` / `#else` / `#endif` with C
// integer expressions and `defined`, `#error`; `#pragma`, `#line`,
// `#warning` and `#ident` are ignored. Include depth, macro expansion
// depth and the text read are bounded: the input may be hostile (ADR-0112).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ─── Errors and tokens ────────────────────────────────────────────────────────

/// A place in the sources: file index and line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loc {
    pub file: usize,
    pub line: usize,
}

#[derive(Debug)]
pub struct Error {
    pub loc: Option<Loc>,
    pub msg: String,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn err<T>(loc: Loc, msg: impl Into<String>) -> Result<T> {
    Err(Error { loc: Some(loc), msg: msg.into() })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Ident(String),
    /// A number and whether it had an L suffix.
    Num(i64, bool),
    Str(Vec<u8>),
    Punct(&'static str),
}

#[derive(Debug, Clone)]
pub struct Tok {
    pub kind: Kind,
    /// The token as written (unquoted file names are joined from these).
    pub text: String,
    pub loc: Loc,
    /// Whitespace came before it (unquoted file names stop there).
    pub space_before: bool,
}

impl Tok {
    pub fn is_punct(&self, p: &str) -> bool { matches!(&self.kind, Kind::Punct(q) if *q == p) }
    pub fn ident(&self) -> Option<&str> { if let Kind::Ident(s) = &self.kind { Some(s) } else { None } }
    pub fn is_keyword(&self, k: &str) -> bool { self.ident().is_some_and(|s| s.eq_ignore_ascii_case(k)) }
}

pub const PUNCTS: &[&str] = &[
    "<<", ">>", "&&", "||", "==", "!=", "<=", ">=", "|", "&", "^", "+", "-", "*", "/", "%", "~", "!",
    "(", ")", ",", "<", ">", "=", ".", ":", "?", "{", "}", "[", "]", ";", "#", "\\", "@", "$",
];

/// Tokens of one logical line (comments already removed).
pub fn tokenize(line: &str, loc: Loc) -> Result<Vec<Tok>> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut space = true;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            space = true;
            i += 1;
            continue;
        }
        let start = i;
        let kind = if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
            Kind::Ident(line[start..i].to_string())
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric()) { i += 1; }
            let raw = &line[start..i];
            let digits = raw.trim_end_matches(['l', 'L', 'u', 'U']);
            let long = raw[digits.len()..].contains(['l', 'L']);
            let value = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
                i64::from_str_radix(h, 16).ok()
            } else if digits.len() > 1 && digits.starts_with('0') {
                i64::from_str_radix(&digits[1..], 8).ok()
            } else {
                digits.parse().ok()
            };
            match value {
                Some(v) => Kind::Num(v, long),
                None => return err(loc, format!("bad number {raw}")),
            }
        } else if c == b'"' {
            let (bytes, end) = read_quoted(b, i, b'"', loc)?;
            i = end;
            Kind::Str(bytes)
        } else if c == b'\'' {
            let (bytes, end) = read_quoted(b, i, b'\'', loc)?;
            i = end;
            Kind::Num(bytes.first().copied().map_or(0, i64::from), false)
        } else if let Some(p) = PUNCTS.iter().find(|p| line[i..].starts_with(**p)) {
            i += p.len();
            Kind::Punct(p)
        } else {
            return err(loc, format!("unexpected character {:?}", c as char));
        };
        out.push(Tok { kind, text: line[start..i].to_string(), loc, space_before: space });
        space = false;
    }
    Ok(out)
}

/// A quoted string or character starting at `b[start]`; its bytes with
/// the escapes resolved, and where it ends.
pub fn read_quoted(b: &[u8], start: usize, quote: u8, loc: Loc) -> Result<(Vec<u8>, usize)> {
    let mut i = start + 1;
    let mut out = Vec::new();
    while i < b.len() && b[i] != quote {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let e = b[i];
        i += 1;
        out.push(match e {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'x' | b'X' => {
                let s = i;
                while i < b.len() && i < s + 2 && b[i].is_ascii_hexdigit() { i += 1; }
                u8::from_str_radix(std::str::from_utf8(&b[s..i]).unwrap_or("0"), 16).unwrap_or(0)
            }
            b'0'..=b'7' => {
                let s = i - 1;
                while i < b.len() && i < s + 3 && (b'0'..=b'7').contains(&b[i]) { i += 1; }
                u8::from_str_radix(std::str::from_utf8(&b[s..i]).unwrap_or("0"), 8).unwrap_or(0)
            }
            other => other,
        });
    }
    if i >= b.len() {
        return err(loc, "a string runs past the end of the line");
    }
    Ok((out, i + 1))
}

// ─── Preprocessor ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Macro {
    pub params: Option<Vec<String>>,
    pub body: Vec<Tok>,
}

/// One #if level: whether the enclosing text is live, whether this branch
/// is, and whether a branch of it has been taken.
pub struct Cond {
    pub parent_live: bool,
    pub live: bool,
    pub taken: bool,
}

pub struct Preprocessor {
    pub macros: HashMap<String, Macro>,
    pub include_dirs: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub out: Vec<Tok>,
    pub depth: usize,
    /// Whether `#include` (and the include keyword) may read files: not
    /// while fuzzing.
    pub reads_files: bool,
    /// Whether an included file emits its lines (else only its
    /// directives count, as a C header's in a resource script).
    pub emits: fn(&Path) -> bool,
    /// A statement that includes a file as `#include` does (os2rc's
    /// `RCINCLUDE`).
    pub include_keyword: Option<&'static str>,
    /// Bytes read so far, from every file (bounded by [`MAX_INPUT`]).
    pub read: usize,
}

/// The most source text one run reads, its includes counted.
pub const MAX_INPUT: usize = 16 << 20;

pub const MAX_INCLUDE_DEPTH: usize = 32;
pub const MAX_EXPANSION_DEPTH: usize = 64;

/// Logical lines of a file: comments removed (inside strings they stay),
/// backslash-newline joined; each with the number of its first line.
pub fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut first_line = 1;
    let mut line = 1;
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_block = false;
    while i < b.len() {
        let c = b[i];
        if in_block {
            if c == '*' && b.get(i + 1) == Some(&'/') {
                in_block = false;
                cur.push(' ');
                i += 2;
            } else {
                if c == '\n' { line += 1; }
                i += 1;
            }
            continue;
        }
        match c {
            '/' if b.get(i + 1) == Some(&'*') => { in_block = true; i += 2; }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' { i += 1; }
            }
            '"' | '\'' => {
                cur.push(c);
                i += 1;
                while i < b.len() && b[i] != c && b[i] != '\n' {
                    if b[i] == '\\' && i + 1 < b.len() && b[i + 1] != '\n' {
                        cur.push(b[i]);
                        i += 1;
                    }
                    cur.push(b[i]);
                    i += 1;
                }
                if i < b.len() && b[i] == c { cur.push(c); i += 1; }
            }
            '\\' if b.get(i + 1) == Some(&'\n') || (b.get(i + 1) == Some(&'\r') && b.get(i + 2) == Some(&'\n')) => {
                i += if b[i + 1] == '\r' { 3 } else { 2 };
                line += 1;
            }
            '\n' => {
                lines.push((first_line, std::mem::take(&mut cur)));
                line += 1;
                first_line = line;
                i += 1;
            }
            '\r' => { i += 1; }
            _ => { cur.push(c); i += 1; }
        }
    }
    if !cur.trim().is_empty() {
        lines.push((first_line, cur));
    }
    lines
}

impl Preprocessor {
    /// A preprocessor searching `include_dirs`, with each of `predefined`
    /// defined as 1.
    pub fn new(include_dirs: Vec<PathBuf>, predefined: &[&str]) -> Self {
        let mut macros = HashMap::new();
        for &name in predefined {
            macros.insert(name.to_string(), Macro { params: None,
                body: vec![Tok { kind: Kind::Num(1, false), text: "1".into(), loc: Loc::default(), space_before: true }] });
        }
        Preprocessor { macros, include_dirs, files: Vec::new(), out: Vec::new(), depth: 0, reads_files: true,
                       emits: |_| true, include_keyword: None, read: 0 }
    }

    pub fn define_cmdline(&mut self, def: &str) -> Result<()> {
        let (name, value) = def.split_once('=').unwrap_or((def, "1"));
        let body = tokenize(value, Loc::default())?;
        self.macros.insert(name.to_string(), Macro { params: None, body });
        Ok(())
    }

    /// Where `name` is: next to `from` (quoted includes), then on the
    /// include path.
    pub fn find(&self, name: &str, from: Option<&Path>, quoted: bool) -> Option<PathBuf> {
        if !self.reads_files {
            return None;
        }
        let mut candidates = Vec::new();
        if quoted {
            if let Some(dir) = from.and_then(Path::parent) { candidates.push(dir.join(name)); }
            candidates.push(PathBuf::from(name));
        }
        candidates.extend(self.include_dirs.iter().map(|d| d.join(name)));
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Preprocess `path`; it emits its lines when `script`, else only its
    /// directives count.
    pub fn run_file(&mut self, path: &Path, script: bool, at: Option<Loc>) -> Result<()> {
        if self.depth >= MAX_INCLUDE_DEPTH {
            return Err(Error { loc: at, msg: "#include nested too deeply".into() });
        }
        let text = std::fs::read(path).map_err(|e| Error { loc: at, msg: format!("{}: {e}", path.display()) })?;
        self.run_text(&text, path, script, at)
    }

    /// Preprocess `text`, the contents of `path` (see [`Self::run_file`]).
    pub fn run_text(&mut self, text: &[u8], path: &Path, script: bool, at: Option<Loc>) -> Result<()> {
        if self.depth >= MAX_INCLUDE_DEPTH {
            return Err(Error { loc: at, msg: "#include nested too deeply".into() });
        }
        self.read = self.read.saturating_add(text.len());
        if self.read > MAX_INPUT {
            return Err(Error { loc: at, msg: "the sources are too large".into() });
        }
        let text = String::from_utf8_lossy(text).into_owned();
        let file = self.files.len();
        self.files.push(path.to_path_buf());
        self.depth += 1;
        let mut conds: Vec<Cond> = Vec::new();
        for (line, content) in logical_lines(&text) {
            let loc = Loc { file, line };
            let live = conds.last().is_none_or(|c| c.live);
            let trimmed = content.trim_start();
            if let Some(directive) = trimmed.strip_prefix('#') {
                self.directive(directive.trim_start(), loc, &mut conds, live, path)?;
                continue;
            }
            if !live || !script { continue; }
            let toks = tokenize(&content, loc)?;
            if let Some(k) = self.include_keyword && toks.first().is_some_and(|t| t.is_keyword(k)) {
                let name: String = toks[1..].iter().map(|t| match &t.kind {
                    Kind::Str(s) => String::from_utf8_lossy(s).into_owned(),
                    _ => t.text.clone(),
                }).collect();
                let found = self.find(name.trim(), Some(path), true)
                    .ok_or_else(|| Error { loc: Some(loc), msg: format!("cannot find {name}") })?;
                self.run_file(&found, true, Some(loc))?;
                continue;
            }
            let expanded = self.expand(&toks, 0)?;
            self.out.extend(expanded);
        }
        self.depth -= 1;
        if !conds.is_empty() {
            return err(Loc { file, line: 0 }, "#if without #endif");
        }
        Ok(())
    }

    pub fn directive(&mut self, d: &str, loc: Loc, conds: &mut Vec<Cond>, live: bool, path: &Path) -> Result<()> {
        let name: String = d.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        let rest = d[name.len()..].trim();
        match name.as_str() {
            "if" | "ifdef" | "ifndef" => {
                let value = if !live { false } else {
                    match name.as_str() {
                        "ifdef" => self.macros.contains_key(first_word(rest)),
                        "ifndef" => !self.macros.contains_key(first_word(rest)),
                        _ => self.condition(rest, loc)?,
                    }
                };
                conds.push(Cond { parent_live: live, live: value, taken: value });
            }
            "elif" => {
                let Some(top) = conds.last() else { return err(loc, "#elif without #if") };
                let (parent, taken) = (top.parent_live, top.taken);
                let value = parent && !taken && self.condition(rest, loc)?;
                let top = conds.last_mut().expect("checked above");
                top.live = value;
                top.taken |= value;
            }
            "else" => {
                let Some(top) = conds.last_mut() else { return err(loc, "#else without #if") };
                top.live = top.parent_live && !top.taken;
                top.taken = true;
            }
            "endif" => {
                if conds.pop().is_none() { return err(loc, "#endif without #if"); }
            }
            _ if !live => {}
            "define" => {
                let mname = first_word(rest).to_string();
                if mname.is_empty() { return err(loc, "#define without a name"); }
                let after = &rest[mname.len()..];
                let (params, body) = if let Some(p) = after.strip_prefix('(') {
                    let close = p.find(')').ok_or(Error { loc: Some(loc), msg: "unclosed macro parameters".into() })?;
                    let params = p[..close].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                    (Some(params), &p[close + 1..])
                } else {
                    (None, after)
                };
                let body = tokenize(body, loc)?;
                self.macros.insert(mname, Macro { params, body });
            }
            "undef" => { self.macros.remove(first_word(rest)); }
            "include" => {
                let (quoted, inner) = if let Some(r) = rest.strip_prefix('"') {
                    (true, r.split('"').next().unwrap_or(""))
                } else if let Some(r) = rest.strip_prefix('<') {
                    (false, r.split('>').next().unwrap_or(""))
                } else {
                    return err(loc, "#include needs \"file\" or <file>");
                };
                let found = self.find(inner, Some(path), quoted)
                    .ok_or_else(|| Error { loc: Some(loc), msg: format!("cannot find include file {inner}") })?;
                let script = (self.emits)(&found);
                self.run_file(&found, script, Some(loc))?;
            }
            "error" => return err(loc, format!("#error {rest}")),
            "pragma" | "line" | "warning" | "ident" | "" => {}
            other => return err(loc, format!("unknown directive #{other}")),
        }
        Ok(())
    }

    /// The value of an #if expression: `defined` resolved, macros
    /// expanded, other names 0.
    pub fn condition(&self, text: &str, loc: Loc) -> Result<bool> {
        let toks = tokenize(text, loc)?;
        let mut resolved = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            if toks[i].ident() == Some("defined") {
                let (name, next) = if toks.get(i + 1).is_some_and(|t| t.is_punct("(")) {
                    (toks.get(i + 2).and_then(Tok::ident).unwrap_or(""), i + 4)
                } else {
                    (toks.get(i + 1).and_then(Tok::ident).unwrap_or(""), i + 2)
                };
                let v = i64::from(self.macros.contains_key(name));
                resolved.push(Tok { kind: Kind::Num(v, false), text: v.to_string(), loc, space_before: true });
                i = next;
            } else {
                resolved.push(toks[i].clone());
                i += 1;
            }
        }
        let expanded = self.expand(&resolved, 0)?;
        let mut ev = Eval { toks: &expanded, pos: 0, unknown_is_zero: true };
        let v = ev.ternary()?;
        if ev.pos != expanded.len() {
            return err(loc, "bad #if expression");
        }
        Ok(v.0 != 0)
    }

    /// Expand the macros in `toks`.
    pub fn expand(&self, toks: &[Tok], depth: usize) -> Result<Vec<Tok>> {
        self.expand_hiding(toks, depth, &mut Vec::new())
    }

    /// [`Preprocessor::expand`], leaving alone the macros in `hidden` (the
    /// ones being expanded: a macro does not expand inside itself).
    pub fn expand_hiding(&self, toks: &[Tok], depth: usize, hidden: &mut Vec<String>) -> Result<Vec<Tok>> {
        if depth > MAX_EXPANSION_DEPTH {
            return err(toks.first().map_or(Loc::default(), |t| t.loc), "macro expansion nested too deeply");
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            let Some(m) = t.ident().filter(|n| !hidden.iter().any(|h| h == n)).and_then(|n| self.macros.get(n)) else {
                out.push(t.clone());
                i += 1;
                continue;
            };
            let name = t.ident().unwrap_or_default().to_string();
            let mut body = match &m.params {
                None => { i += 1; m.body.clone() }
                Some(params) => {
                    if !toks.get(i + 1).is_some_and(|n| n.is_punct("(")) {
                        out.push(t.clone());
                        i += 1;
                        continue;
                    }
                    let (args, next) = collect_args(toks, i + 2, t.loc)?;
                    i = next;
                    let mut body = Vec::new();
                    for b in &m.body {
                        match b.ident().and_then(|n| params.iter().position(|p| p == n)) {
                            Some(k) => body.extend(args.get(k).cloned().unwrap_or_default()),
                            None => body.push(b.clone()),
                        }
                    }
                    body
                }
            };
            for b in &mut body { b.loc = t.loc; }
            hidden.push(name);
            let expanded = self.expand_hiding(&body, depth + 1, hidden);
            hidden.pop();
            let mut expanded = expanded?;
            if let Some(first) = expanded.first_mut() { first.space_before = t.space_before; }
            out.extend(expanded);
        }
        Ok(out)
    }
}

pub fn first_word(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

/// A function-like macro's arguments from `toks[start..]` (after the
/// opening parenthesis); the arguments and the index past the closing one.
pub fn collect_args(toks: &[Tok], start: usize, loc: Loc) -> Result<(Vec<Vec<Tok>>, usize)> {
    let mut args = vec![Vec::new()];
    let mut depth = 0;
    let mut i = start;
    while i < toks.len() {
        let t = &toks[i];
        if t.is_punct("(") { depth += 1; }
        if t.is_punct(")") {
            if depth == 0 { return Ok((args, i + 1)); }
            depth -= 1;
        }
        if t.is_punct(",") && depth == 0 {
            args.push(Vec::new());
        } else {
            args.last_mut().expect("never empty").push(t.clone());
        }
        i += 1;
    }
    err(loc, "unclosed macro arguments")
}

// ─── Expressions ──────────────────────────────────────────────────────────────

/// C integer expressions over tokens; a value and whether a literal in it
/// had an L suffix (RCDATA and CTLDATA then write 4 bytes).
pub struct Eval<'a> {
    pub toks: &'a [Tok],
    pub pos: usize,
    /// #if: a name left after expansion is 0; in resources it is an error.
    pub unknown_is_zero: bool,
}

pub type Val = (i64, bool);

impl Eval<'_> {
    pub fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    pub fn at(&self, p: &str) -> bool { self.peek().is_some_and(|t| t.is_punct(p)) }
    pub fn loc(&self) -> Loc { self.peek().or(self.toks.last()).map_or(Loc::default(), |t| t.loc) }

    pub fn ternary(&mut self) -> Result<Val> {
        let c = self.binary(0)?;
        if !self.at("?") { return Ok(c); }
        self.pos += 1;
        let a = self.ternary()?;
        if !self.at(":") { return err(self.loc(), "expected :"); }
        self.pos += 1;
        let b = self.ternary()?;
        Ok(if c.0 != 0 { a } else { b })
    }

    /// Binary operators by C precedence, lowest first.
    pub fn binary(&mut self, level: usize) -> Result<Val> {
        const LEVELS: &[&[&str]] = &[&["||"], &["&&"], &["|"], &["^"], &["&"], &["==", "!="],
            &["<", ">", "<=", ">="], &["<<", ">>"], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() { return self.unary(); }
        let mut lhs = self.binary(level + 1)?;
        while let Some(op) = self.peek().and_then(|t| match &t.kind {
            Kind::Punct(p) if LEVELS[level].contains(p) => Some(*p),
            _ => None,
        }) {
            let loc = self.loc();
            self.pos += 1;
            let rhs = self.binary(level + 1)?;
            let (a, b) = (lhs.0, rhs.0);
            let v = match op {
                "||" => i64::from(a != 0 || b != 0),
                "&&" => i64::from(a != 0 && b != 0),
                "|" => a | b,
                "^" => a ^ b,
                "&" => a & b,
                "==" => i64::from(a == b),
                "!=" => i64::from(a != b),
                "<" => i64::from(a < b),
                ">" => i64::from(a > b),
                "<=" => i64::from(a <= b),
                ">=" => i64::from(a >= b),
                "<<" => a.wrapping_shl(b as u32),
                ">>" => a.wrapping_shr(b as u32),
                "+" => a.wrapping_add(b),
                "-" => a.wrapping_sub(b),
                "*" => a.wrapping_mul(b),
                "/" | "%" if b == 0 => return err(loc, "division by zero"),
                "/" => a.wrapping_div(b),
                _ => a.wrapping_rem(b),
            };
            lhs = (v, lhs.1 || rhs.1);
        }
        Ok(lhs)
    }

    pub fn unary(&mut self) -> Result<Val> {
        for op in ["-", "+", "~", "!"] {
            if self.at(op) {
                self.pos += 1;
                let (v, l) = self.unary()?;
                return Ok((match op { "-" => v.wrapping_neg(), "+" => v, "~" => !v, _ => i64::from(v == 0) }, l));
            }
        }
        self.primary()
    }

    pub fn primary(&mut self) -> Result<Val> {
        let loc = self.loc();
        let Some(t) = self.peek().cloned() else { return err(loc, "expected a number") };
        self.pos += 1;
        match t.kind {
            Kind::Num(v, l) => Ok((v, l)),
            Kind::Punct("(") => {
                // A cast: "(TYPE)" before an operand.
                if let (Some(ty), Some(close)) = (self.toks.get(self.pos), self.toks.get(self.pos + 1))
                    && ty.ident().is_some() && close.is_punct(")")
                    && self.toks.get(self.pos + 2).is_some_and(|n| matches!(n.kind, Kind::Num(..) | Kind::Ident(_))
                        || n.is_punct("(") || n.is_punct("-") || n.is_punct("~")) {
                    self.pos += 2;
                    return self.unary();
                }
                let v = self.ternary()?;
                if !self.at(")") { return err(self.loc(), "expected )"); }
                self.pos += 1;
                Ok(v)
            }
            Kind::Ident(name) if self.unknown_is_zero => { let _ = name; Ok((0, false)) }
            Kind::Ident(name) => err(loc, format!("unknown name {name} (a missing #define or #include?)")),
            _ => err(loc, format!("expected a number, found {}", t.text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_lines_drop_comments_and_join_continuations() {
        let lines = logical_lines("a /* b\n c */ d // e\n\"/* kept */\" \\\n f\n");
        assert_eq!(lines, [(1, "a   d ".to_string()), (3, "\"/* kept */\"  f".to_string())]);
    }

    /// Predefined names, a parameterised macro, #if with defined, and
    /// headers that only contribute directives.
    #[test]
    fn macros_and_conditions_expand() {
        let mut pre = Preprocessor::new(Vec::new(), &["__TOOL__"]);
        let text = b"#define ADD(a, b) a + b\n#if defined(__TOOL__) && ADD(1, 2) == 3\nyes ADD(x, 4)\n#else\nno\n#endif\n";
        pre.run_text(text, Path::new("t.idl"), true, None).unwrap();
        let words: Vec<&str> = pre.out.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, ["yes", "x", "+", "4"]);
    }

    /// Hostile input ends in errors: deep includes are refused when no file
    /// may be read, unbalanced conditionals and runaway text are caught.
    #[test]
    fn bad_input_is_refused() {
        let mut pre = Preprocessor::new(Vec::new(), &[]);
        pre.reads_files = false;
        assert!(pre.run_text(b"#include \"x.h\"\n", Path::new("t"), true, None).is_err());
        let mut pre = Preprocessor::new(Vec::new(), &[]);
        assert!(pre.run_text(b"#if 1\n", Path::new("t"), true, None).is_err());
        let mut pre = Preprocessor::new(Vec::new(), &[]);
        pre.read = MAX_INPUT;
        assert!(pre.run_text(b"x\n", Path::new("t"), true, None).is_err());
        let mut pre = Preprocessor::new(Vec::new(), &[]);
        assert!(pre.run_text(b"#define A A B\n#define B A\nA\n", Path::new("t"), true, None).is_ok(), "no self-expansion");
    }
}
