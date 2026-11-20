// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX tokens and clauses (ADR-0083). A program is a sequence of clauses:
// a line ends one unless its last token is a comma (continuation), and a
// `;` ends one too. Comments `/* … */` nest and may span lines. Every
// token records whether a blank came before it: between two terms a blank
// is the concatenation operator.

/// A token's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    /// A symbol as written (case kept): a name, a number or a constant.
    Symbol(Vec<u8>),
    /// A literal string's value (quotes, hex and binary resolved).
    Str(Vec<u8>),
    /// An operator: `+ - * / % // ** || & && | \ = \= <> >< > < >= <= \> \<
    /// == \== >> << >>= <<= \>> \<<`.
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    Colon,
}

/// A token and whether blanks (or a comment) came before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub space_before: bool,
}

/// A clause: its tokens and the line it starts on (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub line: usize,
    pub tokens: Vec<Token>,
}

/// A syntax error found while tokenizing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// The REXX error number (6 unmatched comment or quote, 13 invalid
    /// character, 15 invalid hexadecimal or binary string).
    pub code: u32,
    pub line: usize,
}

/// Whether `b` can be part of a symbol.
pub fn is_symbol_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'!' | b'?' | b'_' | b'@' | b'#' | b'$')
}

/// The operators, longest first so the longest match wins.
const OPERATORS: [&str; 30] = [
    "\\==", "\\>>", "\\<<", ">>=", "<<=", "**", "//", "||", "&&", "\\=", "<>", "><", ">=", "<=", "\\>",
    "\\<", "==", ">>", "<<", "+", "-", "*", "/", "%", "&", "|", "\\", "=", ">", "<",
];

/// NOT as OS/2's codepages write it (`¬` is 0xAA in CP437 and CP850); it
/// spells the same operators as `\`.
const NOT_SIGN: u8 = 0xAA;

/// Split `src` into clauses.
pub fn tokenize(src: &[u8]) -> Result<Vec<Clause>, LexError> {
    let mut clauses = Vec::new();
    let mut tokens: Vec<Token> = Vec::new();
    let mut clause_line = 1;
    let mut line = 1;
    let mut space = false;
    let mut i = 0;
    let finish = |tokens: &mut Vec<Token>, clauses: &mut Vec<Clause>, at: usize| {
        if !tokens.is_empty() {
            clauses.push(Clause { line: at, tokens: std::mem::take(tokens) });
        }
    };
    while i < src.len() {
        let b = src[i];
        if tokens.is_empty() { clause_line = line; }
        match b {
            b'\n' => {
                // A comma ending the line continues the clause.
                if matches!(tokens.last(), Some(Token { tok: Tok::Comma, .. })) {
                    tokens.pop();
                    space = true;
                } else {
                    finish(&mut tokens, &mut clauses, clause_line);
                    space = false;
                }
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' | 0x0C => { space = true; i += 1; }
            b';' => {
                finish(&mut tokens, &mut clauses, clause_line);
                space = false;
                i += 1;
            }
            b'/' if src.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                let start_line = line;
                while i < src.len() {
                    if src[i] == b'/' && src.get(i + 1) == Some(&b'*') { depth += 1; i += 2; continue; }
                    if src[i] == b'*' && src.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 { break; }
                        continue;
                    }
                    if src[i] == b'\n' { line += 1; }
                    i += 1;
                }
                if depth != 0 { return Err(LexError { code: 6, line: start_line }); }
                space = true;
            }
            b'\'' | b'"' => {
                let quote = b;
                let start_line = line;
                let mut value = Vec::new();
                i += 1;
                loop {
                    match src.get(i) {
                        None | Some(b'\n') => return Err(LexError { code: 6, line: start_line }),
                        Some(&c) if c == quote => {
                            if src.get(i + 1) == Some(&quote) { value.push(quote); i += 2; } else { i += 1; break; }
                        }
                        Some(&c) => { value.push(c); i += 1; }
                    }
                }
                // A following X or B (not part of a longer symbol) makes it
                // a hexadecimal or binary string.
                let suffix = src.get(i).map(|c| c.to_ascii_uppercase());
                let alone = src.get(i + 1).is_none_or(|&c| !is_symbol_char(c));
                let value = match suffix {
                    Some(b'X') if alone => { i += 1; hex_string(&value).ok_or(LexError { code: 15, line })? }
                    Some(b'B') if alone => { i += 1; bin_string(&value).ok_or(LexError { code: 15, line })? }
                    _ => value,
                };
                tokens.push(Token { tok: Tok::Str(value), space_before: space });
                space = false;
            }
            b'(' | b')' | b',' | b':' => {
                let tok = match b { b'(' => Tok::LParen, b')' => Tok::RParen, b',' => Tok::Comma, _ => Tok::Colon };
                tokens.push(Token { tok, space_before: space });
                space = false;
                i += 1;
            }
            _ if is_symbol_char(b) => {
                let start = i;
                while i < src.len() && is_symbol_char(src[i]) {
                    i += 1;
                    // A number's exponent sign: 1E+5, .5e-3.
                    let sym = &src[start..i];
                    if matches!(src.get(i), Some(b'+' | b'-'))
                        && matches!(sym.last(), Some(b'e' | b'E'))
                        && (sym[0].is_ascii_digit() || sym[0] == b'.')
                        && sym[..sym.len() - 1].iter().all(|c| c.is_ascii_digit() || *c == b'.')
                        && src.get(i + 1).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                }
                tokens.push(Token { tok: Tok::Symbol(src[start..i].to_vec()), space_before: space });
                space = false;
            }
            _ => {
                // `¬` reads as `\` (so `¬=` is `\=`).
                let rest: Vec<u8> = src[i..src.len().min(i + 3)].iter()
                    .map(|&c| if c == NOT_SIGN { b'\\' } else { c }).collect();
                let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(op.as_bytes())) else {
                    return Err(LexError { code: 13, line });
                };
                i += op.len();
                let op: &'static str = op;
                tokens.push(Token { tok: Tok::Op(op), space_before: space });
                space = false;
            }
        }
    }
    finish(&mut tokens, &mut clauses, clause_line);
    Ok(clauses)
}

/// A hexadecimal string's bytes: pairs of hex digits, blanks allowed
/// between bytes (a leading odd digit stands alone).
fn hex_string(s: &[u8]) -> Option<Vec<u8>> {
    let groups: Vec<&[u8]> = s.split(|&c| c == b' ').filter(|g| !g.is_empty()).collect();
    if s.first() == Some(&b' ') || s.last() == Some(&b' ') { return None; }
    let mut digits: Vec<u8> = Vec::new();
    for (k, g) in groups.iter().enumerate() {
        // Every group but the first has whole bytes.
        if k > 0 && g.len() % 2 != 0 { return None; }
        for &c in g.iter() { digits.push((c as char).to_digit(16)? as u8); }
    }
    if digits.len() % 2 == 1 { digits.insert(0, 0); }
    Some(digits.chunks(2).map(|p| p[0] * 16 + p[1]).collect())
}

/// A binary string's bytes: groups of four bits, blanks allowed between
/// groups (the first group may be shorter).
fn bin_string(s: &[u8]) -> Option<Vec<u8>> {
    if s.first() == Some(&b' ') || s.last() == Some(&b' ') { return None; }
    let groups: Vec<&[u8]> = s.split(|&c| c == b' ').filter(|g| !g.is_empty()).collect();
    let mut bits: Vec<u8> = Vec::new();
    for (k, g) in groups.iter().enumerate() {
        if k > 0 && g.len() != 4 { return None; }
        for &c in g.iter() {
            match c { b'0' => bits.push(0), b'1' => bits.push(1), _ => return None }
        }
    }
    while !bits.len().is_multiple_of(8) { bits.insert(0, 0); }
    Some(bits.chunks(8).map(|byte| byte.iter().fold(0u8, |acc, &bit| acc * 2 + bit)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Vec<Tok>> {
        tokenize(src.as_bytes()).unwrap().into_iter().map(|c| c.tokens.into_iter().map(|t| t.tok).collect()).collect()
    }
    fn sym(s: &str) -> Tok { Tok::Symbol(s.as_bytes().to_vec()) }
    fn st(s: &str) -> Tok { Tok::Str(s.as_bytes().to_vec()) }

    #[test]
    fn clauses_end_at_lines_and_semicolons() {
        assert_eq!(toks("say 1; say 2\nx = 3"), vec![vec![sym("say"), sym("1")], vec![sym("say"), sym("2")], vec![sym("x"), Tok::Op("="), sym("3")]]);
        assert_eq!(toks("say 1,\n  2"), vec![vec![sym("say"), sym("1"), sym("2")]], "a trailing comma continues");
        assert_eq!(toks("/* a /* nested */ comment\n spanning */ say 'it''s'"), vec![vec![sym("say"), st("it's")]]);
        let c = tokenize(b"\n\nsay x /* c */\nexit").unwrap();
        assert_eq!((c[0].line, c[1].line), (3, 4));
    }

    #[test]
    fn strings_numbers_and_operators() {
        assert_eq!(toks("'41 42'x \"0100 0001\"b 'ab'xy"), vec![vec![st("AB"), st("A"), st("ab"), sym("xy")]]);
        assert_eq!(toks("1E+3 .5e-2 a.b.c 1E"), vec![vec![sym("1E+3"), sym(".5e-2"), sym("a.b.c"), sym("1E")]]);
        assert_eq!(toks("a\\==b>>=c||d**e//f"), vec![vec![sym("a"), Tok::Op("\\=="), sym("b"), Tok::Op(">>="), sym("c"), Tok::Op("||"), sym("d"), Tok::Op("**"), sym("e"), Tok::Op("//"), sym("f")]]);
        assert_eq!(toks("x = e+1"), vec![vec![sym("x"), Tok::Op("="), sym("e"), Tok::Op("+"), sym("1")]], "a variable E is no exponent");
        assert_eq!(toks("f(a, b): \\x"), vec![vec![sym("f"), Tok::LParen, sym("a"), Tok::Comma, sym("b"), Tok::RParen, Tok::Colon, Tok::Op("\\"), sym("x")]]);
        let not = tokenize(b"a \xAA= b \xAAc").unwrap();
        assert_eq!(not[0].tokens[1].tok, Tok::Op("\\="), "codepage NOT");
        assert_eq!(not[0].tokens[3].tok, Tok::Op("\\"));
        let t = tokenize(b"a b(c)").unwrap();
        assert_eq!(t[0].tokens.iter().map(|t| t.space_before).collect::<Vec<_>>(), [false, true, false, false, false]);
    }

    #[test]
    fn errors_have_their_numbers() {
        assert_eq!(tokenize(b"say 'open").unwrap_err().code, 6);
        assert_eq!(tokenize(b"/* open\n\n").unwrap_err(), LexError { code: 6, line: 1 });
        assert_eq!(tokenize(b"say 'GG'x").unwrap_err().code, 15);
        assert_eq!(tokenize(b"say '012'b").unwrap_err().code, 15);
        assert_eq!(tokenize(b"say `x").unwrap_err().code, 13);
    }
}
