// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX's built-in functions (ADR-0083): the string, word, conversion,
// arithmetic and information functions of the language. Each checks its
// arguments (Error 40 for a wrong count or value) and works on bytes.

use std::rc::Rc;

use super::expr::{is_valid_symbol, symbol_expr, Expr};
use super::interp::{arith, error_message, Interp, RexxError, R, MAX_STRING};
use super::number::Num;
use super::program::{Program, Target};

type Args<'a> = &'a [Option<Vec<u8>>];

/// A built-in function.
type Builtin = fn(&mut Interp, &[u8], Args, &Rc<Program>) -> R<Vec<u8>>;

/// The environment selector of VALUE() for the shell's variables.
const ENV_SELECTOR: &[u8] = b"OS2ENVIRONMENT";

/// The largest range RANDOM() takes (max - min).
const MAX_RANDOM_RANGE: i64 = 100_000;

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August",
                            "September", "October", "November", "December"];
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn bad(name: &[u8], why: &str) -> RexxError {
    RexxError::with(40, format!("{}: {why}", String::from_utf8_lossy(name)))
}

/// Call built-in function `name` (upper case), or `None` if there is none.
pub fn call(interp: &mut Interp, name: &[u8], args: Args, prog: &Rc<Program>) -> Option<R<Vec<u8>>> {
    let f: Builtin = match name {
        b"ABBREV" => abbrev, b"ABS" => abs, b"ADDRESS" => address, b"ARG" => arg, b"B2X" => b2x,
        b"BITAND" | b"BITOR" | b"BITXOR" => bits, b"C2D" => c2d, b"C2X" => c2x, b"CENTER" | b"CENTRE" => center,
        b"COMPARE" => compare, b"COPIES" => copies, b"D2C" => d2c, b"D2X" => d2x, b"DATATYPE" => datatype,
        b"DATE" => date, b"DELSTR" => delstr, b"DELWORD" => delword, b"DIGITS" => digits,
        b"ERRORTEXT" => errortext, b"FORM" => form, b"FORMAT" => format, b"FUZZ" => fuzz, b"INSERT" => insert,
        b"LASTPOS" => lastpos, b"LEFT" => left, b"LENGTH" => length, b"MAX" | b"MIN" => maxmin,
        b"OVERLAY" => overlay, b"POS" => pos, b"QUEUED" => queued, b"RANDOM" => random, b"REVERSE" => reverse,
        b"RIGHT" => right, b"SIGN" => sign, b"SOURCELINE" => sourceline, b"SPACE" => space, b"STRIP" => strip,
        b"SUBSTR" => substr, b"SUBWORD" => subword, b"SYMBOL" => symbol, b"TIME" => time, b"TRACE" => trace,
        b"TRANSLATE" => translate, b"TRUNC" => trunc, b"VALUE" => value, b"VERIFY" => verify, b"WORD" => word,
        b"WORDINDEX" => wordindex, b"WORDLENGTH" => wordlength, b"WORDPOS" => wordpos, b"WORDS" => words,
        b"X2B" => x2b, b"X2C" => x2c, b"X2D" => x2d, b"XRANGE" => xrange,
        _ => return None,
    };
    Some(f(interp, name, args, prog))
}

// ── Argument helpers ─────────────────────────────────────────────────────

fn count(name: &[u8], args: Args, min: usize, max: usize) -> R<()> {
    if args.len() > max { return Err(bad(name, "too many arguments")); }
    for (i, a) in args.iter().enumerate() {
        if i < min && a.is_none() { return Err(bad(name, "an argument is missing")); }
    }
    if args.len() < min { return Err(bad(name, "an argument is missing")); }
    Ok(())
}

fn opt(args: Args<'_>, i: usize) -> Option<&[u8]> { args.get(i).and_then(|a| a.as_deref()) }

fn req(args: Args<'_>, i: usize) -> &[u8] { opt(args, i).unwrap_or_default() }

/// A whole-number argument `>= min`, or `default` when omitted.
fn whole_arg(interp: &Interp, name: &[u8], args: Args, i: usize, min: i64, default: i64) -> R<i64> {
    match opt(args, i) {
        None => Ok(default),
        Some(v) => {
            let n = interp.whole(v).map_err(|_| bad(name, "a whole number was needed"))?;
            if n < min { return Err(bad(name, "a number is out of range")); }
            Ok(n)
        }
    }
}

/// A length argument, bounded by the longest string.
fn len_arg(interp: &Interp, name: &[u8], args: Args, i: usize, default: usize) -> R<usize> {
    let n = whole_arg(interp, name, args, i, 0, default as i64)?;
    if n as u64 > MAX_STRING as u64 { return Err(RexxError::with(5, "a string is too long")); }
    Ok(n as usize)
}

/// A pad character argument (one character), or a blank.
fn pad_arg(name: &[u8], args: Args, i: usize) -> R<u8> {
    match opt(args, i) {
        None => Ok(b' '),
        Some([c]) => Ok(*c),
        Some(_) => Err(bad(name, "the pad must be one character")),
    }
}

/// An option argument: its first letter in upper case.
fn option_arg(name: &[u8], args: Args, i: usize, default: u8, allowed: &[u8]) -> R<u8> {
    match opt(args, i) {
        None => Ok(default),
        Some(v) => {
            let c = v.first().map(u8::to_ascii_uppercase).ok_or_else(|| bad(name, "an empty option"))?;
            if allowed.contains(&c) { Ok(c) } else { Err(bad(name, "an unknown option")) }
        }
    }
}

fn num_arg(name: &[u8], args: Args, i: usize) -> R<Num> {
    Num::parse(req(args, i)).ok_or_else(|| bad(name, "a number was needed"))
}

fn whole_out(n: i64) -> Vec<u8> { n.to_string().into_bytes() }

fn bool_out(b: bool) -> Vec<u8> { if b { b"1".to_vec() } else { b"0".to_vec() } }

/// The blank-delimited words of `s`: (start, end) of each.
fn word_spans(s: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        while i < s.len() && s[i] == b' ' { i += 1; }
        if i >= s.len() { break; }
        let start = i;
        while i < s.len() && s[i] != b' ' { i += 1; }
        out.push((start, i));
    }
    out
}

fn padded(mut s: Vec<u8>, len: usize, pad: u8) -> Vec<u8> {
    s.resize(len, pad);
    s
}

// ── Strings ──────────────────────────────────────────────────────────────

fn abbrev(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let (info, ab) = (req(a, 0), req(a, 1));
    let min = len_arg(i, n, a, 2, ab.len())?;
    Ok(bool_out(ab.len() >= min && ab.len() <= info.len() && info.starts_with(ab)))
}

fn center(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let s = req(a, 0);
    let len = len_arg(i, n, a, 1, 0)?;
    let pad = pad_arg(n, a, 2)?;
    if s.len() >= len {
        let cut = (s.len() - len) / 2;
        return Ok(s[cut..cut + len].to_vec());
    }
    let left = (len - s.len()) / 2;
    let mut out = vec![pad; left];
    out.extend_from_slice(s);
    Ok(padded(out, len, pad))
}

fn compare(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let pad = pad_arg(n, a, 2)?;
    let (x, y) = (req(a, 0), req(a, 1));
    for k in 0..x.len().max(y.len()) {
        if x.get(k).copied().unwrap_or(pad) != y.get(k).copied().unwrap_or(pad) { return Ok(whole_out(k as i64 + 1)); }
    }
    Ok(b"0".to_vec())
}

fn copies(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 2)?;
    let k = len_arg(i, n, a, 1, 0)?;
    if req(a, 0).len().saturating_mul(k) > MAX_STRING { return Err(RexxError::with(5, "a string is too long")); }
    Ok(req(a, 0).repeat(k))
}

fn delstr(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let s = req(a, 0);
    let start = whole_arg(i, n, a, 1, 1, 1)? as usize - 1;
    if start >= s.len() { return Ok(s.to_vec()); }
    let len = len_arg(i, n, a, 2, s.len() - start)?;
    let mut out = s[..start].to_vec();
    out.extend_from_slice(&s[(start + len).min(s.len())..]);
    Ok(out)
}

fn insert(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 5)?;
    let (new, target) = (req(a, 0), req(a, 1));
    let at = len_arg(i, n, a, 2, 0)?;
    let len = len_arg(i, n, a, 3, new.len())?;
    let pad = pad_arg(n, a, 4)?;
    let mut out = padded(target.to_vec(), at.max(target.len()), pad);
    out.truncate(at);
    out.extend(padded(new.to_vec(), len, pad));
    if at < target.len() { out.extend_from_slice(&target[at..]); }
    Ok(out)
}

fn lastpos(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let (needle, hay) = (req(a, 0), req(a, 1));
    let start = whole_arg(i, n, a, 2, 1, hay.len() as i64)? as usize;
    if needle.is_empty() || needle.len() > hay.len() { return Ok(b"0".to_vec()); }
    let last = (start.min(hay.len())).saturating_sub(needle.len());
    let found = (0..=last).rev().find(|&k| &hay[k..k + needle.len()] == needle);
    Ok(whole_out(found.map_or(0, |k| k as i64 + 1)))
}

fn left(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let len = len_arg(i, n, a, 1, 0)?;
    let pad = pad_arg(n, a, 2)?;
    let s = req(a, 0);
    Ok(padded(s[..len.min(s.len())].to_vec(), len, pad))
}

fn right(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let len = len_arg(i, n, a, 1, 0)?;
    let pad = pad_arg(n, a, 2)?;
    let s = req(a, 0);
    if s.len() >= len { return Ok(s[s.len() - len..].to_vec()); }
    let mut out = vec![pad; len - s.len()];
    out.extend_from_slice(s);
    Ok(out)
}

fn length(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(whole_out(req(a, 0).len() as i64))
}

fn overlay(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 5)?;
    let (new, target) = (req(a, 0), req(a, 1));
    let at = whole_arg(i, n, a, 2, 1, 1)? as usize - 1;
    let len = len_arg(i, n, a, 3, new.len())?;
    let pad = pad_arg(n, a, 4)?;
    let mut out = padded(target.to_vec(), at.max(target.len()), pad);
    out.truncate(at);
    out.extend(padded(new.to_vec(), len, pad));
    if at + len < target.len() { out.extend_from_slice(&target[at + len..]); }
    Ok(out)
}

fn pos(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let (needle, hay) = (req(a, 0), req(a, 1));
    let start = whole_arg(i, n, a, 2, 1, 1)? as usize - 1;
    if needle.is_empty() || start >= hay.len() { return Ok(b"0".to_vec()); }
    let found = hay[start..].windows(needle.len()).position(|w| w == needle);
    Ok(whole_out(found.map_or(0, |k| (k + start) as i64 + 1)))
}

fn reverse(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(req(a, 0).iter().rev().copied().collect())
}

fn space(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let s = req(a, 0);
    let gap = len_arg(i, n, a, 1, 1)?;
    let pad = pad_arg(n, a, 2)?;
    let mut out = Vec::new();
    for (k, (st, en)) in word_spans(s).into_iter().enumerate() {
        if k > 0 { out.extend(std::iter::repeat_n(pad, gap)); }
        out.extend_from_slice(&s[st..en]);
        if out.len() > MAX_STRING { return Err(RexxError::with(5, "a string is too long")); }
    }
    Ok(out)
}

fn strip(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let s = req(a, 0);
    let opt_c = option_arg(n, a, 1, b'B', b"BLT")?;
    let c = pad_arg(n, a, 2)?;
    let start = if opt_c != b'T' { s.iter().position(|&x| x != c).unwrap_or(s.len()) } else { 0 };
    let end = if opt_c != b'L' { s.iter().rposition(|&x| x != c).map_or(start, |e| e + 1) } else { s.len() };
    Ok(s[start..end.max(start)].to_vec())
}

fn substr(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 4)?;
    let s = req(a, 0);
    let start = whole_arg(i, n, a, 1, 1, 1)? as usize - 1;
    let rest = s.len().saturating_sub(start);
    let len = len_arg(i, n, a, 2, rest)?;
    let pad = pad_arg(n, a, 3)?;
    let piece = if start < s.len() { s[start..(start + len).min(s.len())].to_vec() } else { Vec::new() };
    Ok(padded(piece, len, pad))
}

fn translate(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 4)?;
    let s = req(a, 0);
    if a.len() == 1 { return Ok(s.to_ascii_uppercase()); }
    let out_t = opt(a, 1).unwrap_or_default();
    let in_t: Vec<u8> = match opt(a, 2) { Some(t) => t.to_vec(), None => (0..=255).collect() };
    let pad = pad_arg(n, a, 3)?;
    Ok(s.iter().map(|&c| match in_t.iter().position(|&x| x == c) {
        Some(k) => out_t.get(k).copied().unwrap_or(pad),
        None => c,
    }).collect())
}

fn verify(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 4)?;
    let (s, reference) = (req(a, 0), req(a, 1));
    let matching = option_arg(n, a, 2, b'N', b"MN")? == b'M';
    let start = whole_arg(i, n, a, 3, 1, 1)? as usize - 1;
    let found = s.iter().enumerate().skip(start).find(|(_, c)| reference.contains(c) == matching);
    Ok(whole_out(found.map_or(0, |(k, _)| k as i64 + 1)))
}

fn xrange(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 2)?;
    let one = |i: usize, d: u8| -> R<u8> {
        match opt(a, i) { None => Ok(d), Some([c]) => Ok(*c), Some(_) => Err(bad(n, "one character was needed")) }
    };
    let (from, to) = (one(0, 0)?, one(1, 255)?);
    Ok(if from <= to { (from..=to).collect() } else { (from..=255).chain(0..=to).collect() })
}

fn bits(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let (x, y) = (req(a, 0), opt(a, 1).unwrap_or_default());
    let pad = opt(a, 2).map(|p| if p.len() == 1 { Ok(p[0]) } else { Err(bad(n, "the pad must be one character")) }).transpose()?;
    let op = match n { b"BITAND" => |p: u8, q: u8| p & q, b"BITOR" => |p: u8, q: u8| p | q, _ => |p: u8, q: u8| p ^ q };
    let len = x.len().max(y.len());
    Ok((0..len).map(|k| match (x.get(k), y.get(k), pad) {
        (Some(&p), Some(&q), _) => op(p, q),
        (Some(&p), None, Some(d)) | (None, Some(&p), Some(d)) => op(p, d),
        (Some(&p), None, None) | (None, Some(&p), None) => p,
        (None, None, _) => 0,
    }).collect())
}

// ── Words ────────────────────────────────────────────────────────────────

fn delword(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let s = req(a, 0);
    let spans = word_spans(s);
    let first = whole_arg(i, n, a, 1, 1, 1)? as usize - 1;
    if first >= spans.len() { return Ok(s.to_vec()); }
    let len = len_arg(i, n, a, 2, spans.len())?;
    let from = spans[first].0;
    let last = first + len;
    // Up to the next word kept (its leading blanks go too).
    let to = if last < spans.len() { spans[last].0 } else { s.len() };
    let mut out = s[..from].to_vec();
    out.extend_from_slice(&s[to..]);
    Ok(out)
}

fn subword(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let s = req(a, 0);
    let spans = word_spans(s);
    let first = whole_arg(i, n, a, 1, 1, 1)? as usize - 1;
    if first >= spans.len() { return Ok(Vec::new()); }
    let len = len_arg(i, n, a, 2, spans.len() - first)?;
    if len == 0 { return Ok(Vec::new()); }
    let last = (first + len).min(spans.len()) - 1;
    Ok(s[spans[first].0..spans[last].1].to_vec())
}

fn word(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 2)?;
    let k = whole_arg(i, n, a, 1, 1, 1)? as usize;
    let s = req(a, 0);
    Ok(word_spans(s).get(k - 1).map(|&(st, en)| s[st..en].to_vec()).unwrap_or_default())
}

fn wordindex(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 2)?;
    let k = whole_arg(i, n, a, 1, 1, 1)? as usize;
    Ok(whole_out(word_spans(req(a, 0)).get(k - 1).map_or(0, |&(st, _)| st as i64 + 1)))
}

fn wordlength(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 2)?;
    let k = whole_arg(i, n, a, 1, 1, 1)? as usize;
    Ok(whole_out(word_spans(req(a, 0)).get(k - 1).map_or(0, |&(st, en)| (en - st) as i64)))
}

fn wordpos(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 2, 3)?;
    let (phrase, s) = (req(a, 0), req(a, 1));
    let want: Vec<&[u8]> = word_spans(phrase).into_iter().map(|(x, y)| &phrase[x..y]).collect();
    let have: Vec<&[u8]> = word_spans(s).into_iter().map(|(x, y)| &s[x..y]).collect();
    let start = whole_arg(i, n, a, 2, 1, 1)? as usize - 1;
    if want.is_empty() || want.len() > have.len() { return Ok(b"0".to_vec()); }
    let found = (start..=have.len() - want.len()).find(|&k| have[k..k + want.len()] == want[..]);
    Ok(whole_out(found.map_or(0, |k| k as i64 + 1)))
}

fn words(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(whole_out(word_spans(req(a, 0)).len() as i64))
}

// ── Conversions ──────────────────────────────────────────────────────────

fn hex_digits(s: &[u8], allow_blanks: bool) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for &c in s {
        if c == b' ' && allow_blanks { continue; }
        out.push((c as char).to_digit(16)? as u8);
    }
    Some(out)
}

fn x2c(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let mut d = hex_digits(req(a, 0), true).ok_or_else(|| bad(n, "not hexadecimal"))?;
    if d.len() % 2 == 1 { d.insert(0, 0); }
    Ok(d.chunks(2).map(|p| p[0] * 16 + p[1]).collect())
}

fn c2x(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(req(a, 0).iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect())
}

fn b2x(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let mut bits: Vec<u8> = Vec::new();
    for &c in req(a, 0) {
        match c { b'0' => bits.push(0), b'1' => bits.push(1), b' ' => {}, _ => return Err(bad(n, "not binary")) }
    }
    while !bits.len().is_multiple_of(4) { bits.insert(0, 0); }
    Ok(bits.chunks(4).map(|q| b"0123456789ABCDEF"[q.iter().fold(0usize, |acc, &b| acc * 2 + b as usize)]).collect())
}

fn x2b(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let d = hex_digits(req(a, 0), true).ok_or_else(|| bad(n, "not hexadecimal"))?;
    Ok(d.iter().flat_map(|x| format!("{x:04b}").into_bytes()).collect())
}

/// The unsigned value of bytes (`c2d`, `x2d`), as a REXX number.
fn bytes_value(bytes: &[u8]) -> Num {
    let mut v = Num::zero();
    for &b in bytes {
        v = v.mul(&Num::from_i64(256), super::number::MAX_DIGITS).unwrap_or_else(|_| Num::zero())
            .add(&Num::from_i64(b as i64), super::number::MAX_DIGITS).unwrap_or_else(|_| Num::zero());
    }
    v
}

/// `bytes` as a signed (two's complement) number when `signed`.
fn decode(i: &Interp, n: &[u8], bytes: &[u8], signed: bool) -> R<Vec<u8>> {
    let mut v = bytes_value(bytes);
    if signed && bytes.first().is_some_and(|&b| b & 0x80 != 0) {
        let modulus = Num::from_i64(256).pow(&Num::from_i64(bytes.len() as i64), super::number::MAX_DIGITS).map_err(arith)?;
        v = v.sub(&modulus, super::number::MAX_DIGITS).map_err(arith)?;
    }
    if v.clone().round(i.digits()) != v { return Err(bad(n, "the result needs more digits")); }
    Ok(i.format_num(&v))
}

fn c2d(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let s = req(a, 0);
    match opt(a, 1) {
        None => decode(i, n, s, false),
        Some(_) => {
            let len = len_arg(i, n, a, 1, 0)?;
            if len == 0 { return Ok(b"0".to_vec()); }
            let mut bytes = vec![0u8; len.saturating_sub(s.len())];
            bytes.extend_from_slice(&s[s.len().saturating_sub(len)..]);
            decode(i, n, &bytes, true)
        }
    }
}

fn x2d(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let d = hex_digits(req(a, 0), true).ok_or_else(|| bad(n, "not hexadecimal"))?;
    match opt(a, 1) {
        None => {
            let mut d = d;
            if d.len() % 2 == 1 { d.insert(0, 0); }
            let bytes: Vec<u8> = d.chunks(2).map(|p| p[0] * 16 + p[1]).collect();
            decode(i, n, &bytes, false)
        }
        Some(_) => {
            let len = len_arg(i, n, a, 1, 0)?;
            if len == 0 { return Ok(b"0".to_vec()); }
            let mut digits = vec![0u8; len.saturating_sub(d.len())];
            digits.extend_from_slice(&d[d.len().saturating_sub(len)..]);
            let negative = digits[0] >= 8;
            let mut v = Num::zero();
            for &x in &digits { v = v.mul(&Num::from_i64(16), super::number::MAX_DIGITS).map_err(arith)?.add(&Num::from_i64(x as i64), super::number::MAX_DIGITS).map_err(arith)?; }
            if negative {
                let m = Num::from_i64(16).pow(&Num::from_i64(len as i64), super::number::MAX_DIGITS).map_err(arith)?;
                v = v.sub(&m, super::number::MAX_DIGITS).map_err(arith)?;
            }
            Ok(i.format_num(&v))
        }
    }
}

/// A whole number's two's complement bytes, `len` long (or as many as
/// it needs, for a non-negative one).
fn encode(i: &Interp, n: &[u8], a: Args, len_index: usize) -> R<Vec<u8>> {
    let v = i.whole(req(a, 0)).map_err(|_| bad(n, "a whole number was needed"))?;
    let len = match opt(a, len_index) {
        Some(_) => Some(len_arg(i, n, a, len_index, 0)?.min(64)),
        None => { if v < 0 { return Err(bad(n, "a negative number needs a length")); } None }
    };
    let be = (v as i128).to_be_bytes();
    let bytes = match len {
        Some(l) => {
            let ext = if v < 0 { 0xFF } else { 0 };
            let mut out = vec![ext; l.saturating_sub(16)];
            out.extend_from_slice(&be[16usize.saturating_sub(l)..]);
            out
        }
        None => {
            let first = be.iter().position(|&b| b != 0).unwrap_or(15);
            be[first..].to_vec()
        }
    };
    Ok(bytes)
}

fn d2c(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    encode(i, n, a, 1)
}

fn d2x(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let v = i.whole(req(a, 0)).map_err(|_| bad(n, "a whole number was needed"))?;
    match opt(a, 1) {
        None => {
            if v < 0 { return Err(bad(n, "a negative number needs a length")); }
            Ok(format!("{v:X}").into_bytes())
        }
        Some(_) => {
            let len = len_arg(i, n, a, 1, 0)?.min(32);
            let full = format!("{:032X}", v as i128 as u128);
            Ok(full.as_bytes()[32 - len..].to_vec())
        }
    }
}

// ── Numbers ──────────────────────────────────────────────────────────────

fn abs(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    Ok(i.format_num(&num_arg(n, a, 0)?.abs().round(i.digits())))
}

fn sign(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let v = num_arg(n, a, 0)?.round(i.digits());
    Ok(if v.is_zero() { b"0".to_vec() } else if v.is_negative() { b"-1".to_vec() } else { b"1".to_vec() })
}

fn maxmin(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    if a.is_empty() || a.iter().any(Option::is_none) { return Err(bad(n, "an argument is missing")); }
    let mut best = num_arg(n, a, 0)?;
    for k in 1..a.len() {
        let v = num_arg(n, a, k)?;
        let better = if n == b"MAX" { v.cmp_value(&best).is_gt() } else { v.cmp_value(&best).is_lt() };
        if better { best = v; }
    }
    Ok(i.format_num(&best.round(i.digits())))
}

/// `v` with its fraction cut to `places` decimals, written with exactly
/// that many.
fn fixed(v: &Num, places: usize) -> R<Vec<u8>> {
    let max = super::number::MAX_DIGITS;
    let scale = Num::from_i64(10).pow(&Num::from_i64(places as i64), max).map_err(arith)?;
    let cut = v.mul(&scale, max).map_err(arith)?.trunc();
    let mut digits = cut.abs().format(max);
    if places > 0 {
        while digits.len() <= places { digits.insert(0, b'0'); }
        digits.insert(digits.len() - places, b'.');
    }
    if cut.is_negative() { digits.insert(0, b'-'); }
    Ok(digits)
}

fn trunc(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let v = num_arg(n, a, 0)?.round(i.digits());
    let places = len_arg(i, n, a, 1, 0)?.min(super::number::MAX_DIGITS);
    fixed(&v, places)
}

/// FORMAT(number [, before [, after]]): `after` decimals (rounded), the
/// integer part padded on the left to `before` places.
fn format(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let v = num_arg(n, a, 0)?.round(i.digits());
    let before = opt(a, 1).map(|_| len_arg(i, n, a, 1, 0)).transpose()?;
    let mut s = match opt(a, 2).map(|_| len_arg(i, n, a, 2, 0)).transpose()? {
        Some(places) => {
            let places = places.min(super::number::MAX_DIGITS);
            let half = Num::parse(format!("0.{}5", "0".repeat(places)).as_bytes()).expect("a number");
            let max = super::number::MAX_DIGITS;
            let nudged = if v.is_negative() { v.sub(&half, max) } else { v.add(&half, max) }.map_err(arith)?;
            let mut out = fixed(&nudged, places)?;
            // No minus sign on a value that rounded to zero.
            if out.first() == Some(&b'-') && out[1..].iter().all(|&c| c == b'0' || c == b'.') { out.remove(0); }
            out
        }
        None => i.format_num(&v),
    };
    if let Some(width) = before {
        let int_len = s.iter().position(|&c| c == b'.' || c == b'E').unwrap_or(s.len());
        if int_len > width { return Err(bad(n, "the number does not fit")); }
        let mut out = vec![b' '; width - int_len];
        out.append(&mut s);
        s = out;
    }
    Ok(s)
}

fn random(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 3)?;
    let (min, max) = match (opt(a, 0), opt(a, 1)) {
        (Some(_), None) if a.len() == 1 => (0, whole_arg(i, n, a, 0, 0, 999)?),
        _ => (whole_arg(i, n, a, 0, 0, 0)?, whole_arg(i, n, a, 1, 0, 999)?),
    };
    if max < min || max - min > MAX_RANDOM_RANGE { return Err(bad(n, "a range of at most 100000")); }
    if let Some(seed) = opt(a, 2) {
        i.random_state = i.whole(seed).map_err(|_| bad(n, "a whole number was needed"))? as u64;
    } else if i.random_state == 0 {
        i.random_state = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    }
    // splitmix64.
    i.random_state = i.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = i.random_state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    Ok(whole_out(min + (z % (max - min + 1) as u64) as i64))
}

// ── Information ──────────────────────────────────────────────────────────

fn address(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(i.address().to_vec())
}

fn arg(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 2)?;
    let args = i.args_stack.last().cloned().unwrap_or_default();
    match opt(a, 0) {
        None => {
            if a.len() > 1 { return Err(bad(n, "an option needs an argument number")); }
            let last = args.iter().rposition(Option::is_some).map_or(0, |k| k + 1);
            Ok(whole_out(last as i64))
        }
        Some(_) => {
            let k = whole_arg(i, n, a, 0, 1, 1)? as usize;
            let given = args.get(k - 1).cloned().flatten();
            match option_arg(n, a, 1, b'A', b"AEO")? {
                b'E' => Ok(bool_out(given.is_some())),
                b'O' => Ok(bool_out(given.is_none())),
                _ => Ok(given.unwrap_or_default()),
            }
        }
    }
}

fn datatype(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 2)?;
    let s = req(a, 0);
    let is_num = Num::parse(s).is_some();
    let Some(_) = opt(a, 1) else { return Ok(if is_num { b"NUM".to_vec() } else { b"CHAR".to_vec() }) };
    let t = option_arg(n, a, 1, b'N', b"ABLMNSUWX")?;
    let ok = match t {
        b'A' => !s.is_empty() && s.iter().all(u8::is_ascii_alphanumeric),
        b'B' => { let d: Vec<u8> = s.iter().copied().filter(|&c| c != b' ').collect(); !d.is_empty() && d.iter().all(|&c| c == b'0' || c == b'1') }
        b'L' => !s.is_empty() && s.iter().all(u8::is_ascii_lowercase),
        b'U' => !s.is_empty() && s.iter().all(u8::is_ascii_uppercase),
        b'M' => !s.is_empty() && s.iter().all(u8::is_ascii_alphabetic),
        b'N' => is_num,
        b'S' => is_valid_symbol(s),
        b'W' => Num::parse(s).and_then(|v| v.to_whole(i.digits())).is_some(),
        _ => s.is_empty() || hex_digits(s, true).is_some_and(|d| !d.is_empty()),
    };
    Ok(bool_out(ok))
}

fn digits(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(whole_out(i.digits() as i64))
}

fn fuzz(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(whole_out(i.fuzz() as i64))
}

fn form(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(if i.engineering() { b"ENGINEERING".to_vec() } else { b"SCIENTIFIC".to_vec() })
}

fn errortext(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let k = whole_arg(i, n, a, 0, 0, 0)?;
    if k > 99 { return Err(bad(n, "error numbers go up to 99")); }
    let text = error_message(k as u32);
    Ok(if text == "Error" { Vec::new() } else { text.as_bytes().to_vec() })
}

fn queued(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 0)?;
    Ok(whole_out(i.queue.len() as i64))
}

fn sourceline(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 1)?;
    match opt(a, 0) {
        None => Ok(whole_out(i.source_line_count() as i64)),
        Some(_) => {
            let k = whole_arg(i, n, a, 0, 1, 1)? as usize;
            i.source_line(k).map(<[u8]>::to_vec).ok_or_else(|| bad(n, "no such line"))
        }
    }
}

fn symbol(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 1)?;
    let name = req(a, 0);
    if !is_valid_symbol(name) { return Ok(b"BAD".to_vec()); }
    let upper = name.to_ascii_uppercase();
    let exists = match symbol_expr(&upper) {
        Expr::Lit(_) => false,
        Expr::Var(v) => i.var_exists(&v),
        e @ Expr::Compound { .. } => i.compound_exists(&e)?,
        _ => false,
    };
    Ok(if exists { b"VAR".to_vec() } else { b"LIT".to_vec() })
}

fn trace(_: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 1)?;
    Ok(b"N".to_vec())
}

fn value(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 1, 3)?;
    let name = req(a, 0);
    if let Some(selector) = opt(a, 2) {
        if !selector.eq_ignore_ascii_case(ENV_SELECTOR) { return Err(bad(n, "an unknown selector")); }
        let old = i.host.get_env(name).unwrap_or_default();
        if let Some(new) = opt(a, 1) { i.host.set_env(name, Some(new)); }
        return Ok(old);
    }
    if !is_valid_symbol(name) { return Err(bad(n, "not a symbol")); }
    let e = symbol_expr(&name.to_ascii_uppercase());
    let old = i.symbol_value(&e)?;
    if let Some(new) = opt(a, 1) {
        let target = match e {
            Expr::Var(v) => Target::Var(v),
            c @ Expr::Compound { .. } => Target::Compound(c),
            _ => return Err(bad(n, "a constant cannot be set")),
        };
        i.assign(&target, new.to_vec())?;
    }
    Ok(old)
}

// ── Date and time ────────────────────────────────────────────────────────

/// Days from 1 January 0001 to `y-m-d` (proleptic Gregorian).
fn base_days(y: i64, m: i64, d: i64) -> i64 {
    let y1 = y - 1;
    let mut days = y1 * 365 + y1 / 4 - y1 / 100 + y1 / 400;
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let cum = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    days += cum[(m - 1) as usize] + d - 1;
    if leap && m > 2 { days += 1; }
    days
}

fn date(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 1)?;
    let o = option_arg(n, a, 0, b'N', b"BDELMNOSUW")?;
    let (t, _) = i.host.now().ok_or_else(|| RexxError::with(48, "no clock"))?;
    let (y, m, d) = (t.year as i64, t.month as i64, t.day as i64);
    let month = MONTHS[(m - 1) as usize];
    let s = match o {
        b'B' => base_days(y, m, d).to_string(),
        b'D' => (base_days(y, m, d) - base_days(y, 1, 1) + 1).to_string(),
        b'E' => format!("{d:02}/{m:02}/{:02}", y % 100),
        b'L' => format!("{d} {month} {y}"),
        b'M' => month.to_string(),
        b'O' => format!("{:02}/{m:02}/{d:02}", y % 100),
        b'S' => format!("{y:04}{m:02}{d:02}"),
        b'U' => format!("{m:02}/{d:02}/{:02}", y % 100),
        b'W' => WEEKDAYS[t.weekday as usize % 7].to_string(),
        _ => format!("{d} {} {y}", &month[..3]),
    };
    Ok(s.into_bytes())
}

fn time(i: &mut Interp, n: &[u8], a: Args, _: &Rc<Program>) -> R<Vec<u8>> {
    count(n, a, 0, 1)?;
    let o = option_arg(n, a, 0, b'N', b"CEHLMNRS")?;
    if o == b'E' || o == b'R' {
        let now = std::time::Instant::now();
        let elapsed = i.timer_start.map(|s| now.duration_since(s));
        if o == b'R' || i.timer_start.is_none() { i.timer_start = Some(now); }
        return Ok(match elapsed {
            None => b"0".to_vec(),
            Some(d) => format!("{}.{:06}", d.as_secs(), d.subsec_micros()).into_bytes(),
        });
    }
    let (t, hundredths) = i.host.now().ok_or_else(|| RexxError::with(48, "no clock"))?;
    let (h, m, s) = (t.hours as u32, t.minutes as u32, t.seconds as u32);
    let out = match o {
        b'C' => format!("{}:{m:02}{}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "am" } else { "pm" }),
        b'H' => h.to_string(),
        b'L' => format!("{h:02}:{m:02}:{s:02}.{:06}", hundredths as u32 * 10_000),
        b'M' => (h * 60 + m).to_string(),
        b'S' => (h * 3600 + m * 60 + s).to_string(),
        _ => format!("{h:02}:{m:02}:{s:02}"),
    };
    Ok(out.into_bytes())
}
