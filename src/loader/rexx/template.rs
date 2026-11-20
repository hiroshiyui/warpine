// SPDX-License-Identifier: GPL-3.0-or-later
//
// PARSE templates (ADR-0083). A template splits a string at its patterns:
// a literal pattern is searched for from the current position; an absolute
// position (`n`, `=n`) or a relative one (`+n`, `-n`, counted from where
// the last pattern matched) moves to it. The piece between two patterns
// goes to the targets between them: every one but the last takes a word,
// the last the rest. A position left of (or at) the current one takes the
// piece to the string's end.

use super::interp::{Interp, RexxError, R};
use super::program::{PatternValue, Target, TemplateItem};

/// Parse `s` with `template`, assigning its targets.
pub fn parse(interp: &mut Interp, s: &[u8], template: &[TemplateItem]) -> R<()> {
    let mut start = 0usize;
    let mut last_match = 0usize;
    let mut pending: Vec<Option<&Target>> = Vec::new();
    for item in template {
        match item {
            TemplateItem::Target(t) => pending.push(Some(t)),
            TemplateItem::Placeholder => pending.push(None),
            TemplateItem::Literal(pv) => {
                let pattern = pattern_value(interp, pv)?;
                match find(s, &pattern, start) {
                    Some(k) => {
                        assign_words(interp, &pending, &s[start..k])?;
                        last_match = k;
                        start = k + pattern.len();
                    }
                    None => {
                        assign_words(interp, &pending, &s[start..])?;
                        start = s.len();
                        last_match = s.len();
                    }
                }
                pending.clear();
            }
            TemplateItem::Absolute(pv) | TemplateItem::Relative(_, pv) => {
                let n = interp.whole(&pattern_value(interp, pv)?)?;
                let target = match item {
                    TemplateItem::Absolute(_) => {
                        if n < 1 { return Err(RexxError::with(26, "a position below 1")); }
                        n - 1
                    }
                    TemplateItem::Relative(true, _) => last_match as i64 - n,
                    _ => last_match as i64 + n,
                };
                let t = target.clamp(0, s.len() as i64) as usize;
                let piece = if t > start { &s[start..t] } else { &s[start..] };
                assign_words(interp, &pending, piece)?;
                pending.clear();
                start = t;
                last_match = t;
            }
        }
    }
    assign_words(interp, &pending, &s[start.min(s.len())..])
}

/// A pattern's string: given, or a variable's value.
fn pattern_value(interp: &Interp, pv: &PatternValue) -> R<Vec<u8>> {
    match pv {
        PatternValue::Const(c) => Ok(c.clone()),
        PatternValue::Var(e) => interp.symbol_value(e),
    }
}

/// Where `pattern` occurs in `s` from `from` on; the empty pattern
/// matches at the end.
fn find(s: &[u8], pattern: &[u8], from: usize) -> Option<usize> {
    if pattern.is_empty() { return Some(s.len()); }
    if from > s.len() { return None; }
    s[from..].windows(pattern.len()).position(|w| w == pattern).map(|k| k + from)
}

/// Give `piece` to `targets` (None: a placeholder): one word each, the
/// last the rest; a single target the whole piece.
fn assign_words(interp: &mut Interp, targets: &[Option<&Target>], piece: &[u8]) -> R<()> {
    let mut rest = piece;
    for (i, target) in targets.iter().enumerate() {
        let value: &[u8] = if i + 1 == targets.len() {
            rest
        } else {
            let lead = rest.iter().position(|&c| c != b' ').unwrap_or(rest.len());
            rest = &rest[lead..];
            let end = rest.iter().position(|&c| c == b' ').unwrap_or(rest.len());
            let word = &rest[..end];
            // The blank that ends the word goes with it.
            rest = &rest[(end + 1).min(rest.len())..];
            word
        };
        interp.trace_assigned(target.is_none(), value);
        if let Some(t) = target { interp.assign(t, value.to_vec())?; }
    }
    Ok(())
}
