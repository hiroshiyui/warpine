// SPDX-License-Identifier: GPL-3.0-or-later
//
// TRACE (ADR-0084): the program's clauses and values as it runs, written
// through the host. The settings are A(ll), C(ommands), E(rrors),
// F(ailures), I(ntermediates), L(abels), N(ormal, as F), O(ff) and
// R(esults); a leading `?` (interactive tracing) is accepted and kept,
// but the program does not pause (unverified).
//
// A clause is written as its line number and `*-*` and its text (`*,*`
// for a continuation line, `*~*` for INTERPRET's); values below it as
// `>>>` (a result), `>V>` (a variable), `>L>` (a literal), `>F>` (a
// function), `>O>` (an operation), `>P>` (a prefix operation), `>C>` (a
// compound variable's name), `>.>` (a placeholder in a template), and a
// failed command's `+++ RC(n) +++` (layout unverified).

use std::rc::Rc;

use super::expr::Expr;
use super::interp::{Interp, RexxError, R};
use super::number::Num;
use super::program::{Instr, Program};

/// The settings TRACE takes (their first letter).
const SETTINGS: &[u8] = b"ACEFILNOR";

/// The width of a clause's line number.
const LINE_WIDTH: usize = 6;

impl Interp<'_> {
    /// Change the TRACE setting: `?` toggles interactive tracing, a
    /// letter sets the setting; a whole number (interactive skipping) is
    /// accepted and ignored.
    pub fn set_trace(&mut self, v: &[u8]) -> R<()> {
        let v = v.trim_ascii();
        if Num::parse(v).is_some() { return self.whole(v).map(|_| ()); }
        let mut rest = v;
        while let [b'?', tail @ ..] = rest {
            self.settings.trace_interactive = !self.settings.trace_interactive;
            rest = tail;
        }
        let Some(&c) = rest.first() else { return Ok(()) };
        let c = c.to_ascii_uppercase();
        if !SETTINGS.contains(&c) { return Err(RexxError::with(24, String::from_utf8_lossy(v))); }
        if c == b'O' { self.settings.trace_interactive = false; }
        self.settings.trace = c;
        Ok(())
    }

    /// The TRACE setting as TRACE() gives it: `?` when interactive, and
    /// the letter.
    pub fn trace_setting(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.settings.trace_interactive { out.push(b'?'); }
        out.push(self.settings.trace);
        out
    }

    /// Whether `instr` is traced before it runs.
    pub(super) fn traces_clause(&self, instr: &Instr) -> bool {
        match self.settings.trace {
            b'A' | b'R' | b'I' => !matches!(instr, Instr::Then | Instr::Else),
            b'L' => matches!(instr, Instr::Label(_)),
            b'C' => matches!(instr, Instr::Command(_) | Instr::Address { command: Some(_), .. }),
            _ => false,
        }
    }

    /// Write the clause of instruction `pc`.
    pub(super) fn trace_clause(&mut self, prog: &Rc<Program>, pc: usize) {
        let Some(&(start, end)) = prog.spans.get(pc) else { return };
        let text = prog.source.get(start..end).unwrap_or_default();
        let line = prog.lines[pc];
        for (k, part) in text.split(|&b| b == b'\n').enumerate() {
            let part = part.trim_ascii();
            let mut out = if prog.interpreted {
                format!("{:LINE_WIDTH$} *~* ", "").into_bytes()
            } else if k == 0 {
                format!("{line:LINE_WIDTH$} *-* ").into_bytes()
            } else {
                format!("{:LINE_WIDTH$} *,* ", "").into_bytes()
            };
            out.extend_from_slice(part);
            self.trace_line(&out);
        }
    }

    /// Write a value with its marker (`>>>`, `>V>`, …).
    pub(super) fn trace_value(&mut self, marker: &[u8], value: &[u8]) {
        let mut out = format!("{:LINE_WIDTH$} ", "").into_bytes();
        out.extend_from_slice(marker);
        out.extend_from_slice(b"   \"");
        out.extend_from_slice(value);
        out.push(b'"');
        self.trace_line(&out);
    }

    /// TRACE I: an expression's step and its value.
    pub(super) fn trace_intermediate(&mut self, e: &Expr, value: &[u8]) {
        let marker: &[u8] = match e {
            Expr::Lit(_) => b">L>",
            Expr::Var(_) => b">V>",
            Expr::Compound { .. } => {
                if let Ok(name) = self.compound_name(e) { self.trace_value(b">C>", &name); }
                b">V>"
            }
            Expr::Call { .. } => b">F>",
            Expr::Unary(..) => b">P>",
            Expr::Binary(..) => b">O>",
        };
        self.trace_value(marker, value);
    }

    /// A PARSE target's value (`>>>`), or a placeholder's (`>.>`), under
    /// TRACE R or I.
    pub(super) fn trace_assigned(&mut self, placeholder: bool, value: &[u8]) {
        if matches!(self.settings.trace, b'R' | b'I') {
            self.trace_value(if placeholder { b">.>" } else { b">>>" }, value);
        }
    }

    /// A command that returned `rc` (not 0): its clause, unless already
    /// written, and `+++ RC(rc) +++`, as the setting asks. A `failure`
    /// (negative code) shows under N and F too.
    pub(super) fn trace_command_error(&mut self, rc: &[u8], failure: bool) {
        let t = self.settings.trace;
        let show = match t { b'A' | b'C' | b'E' | b'R' | b'I' => true, b'F' | b'N' => failure, _ => false };
        if !show { return; }
        if !matches!(t, b'A' | b'C' | b'R' | b'I') && let Some((prog, pc)) = self.current.clone() {
            self.trace_clause(&prog, pc);
        }
        let line = format!("{:LINE_WIDTH$} +++ RC({}) +++", "", String::from_utf8_lossy(rc));
        self.trace_line(line.as_bytes());
    }
}
