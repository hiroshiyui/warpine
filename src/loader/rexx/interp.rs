// SPDX-License-Identifier: GPL-3.0-or-later
//
// The REXX interpreter (ADR-0083): runs a program's instructions with its
// variables, routines, queue and settings. The shell it runs in is a
// `Host`: SAY's output, commands, terminal input, interruption and the
// clock go through it.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::expr::{BinOp, Expr, TailPart, UnOp};
use super::lexer::tokenize;
use super::number::{ArithError, Num, MAX_DIGITS};
use super::program::{build, DoSpec, Instr, NameRef, ParseSource, Program, Target};
use crate::loader::clock::LocalTime;

/// The most routines (internal and external) active inside one another.
pub const MAX_CALL_DEPTH: usize = 100;
/// The most INTERPRETs active inside one another.
pub const MAX_INTERPRET_DEPTH: usize = 20;
/// The longest string a script may build.
pub const MAX_STRING: usize = 16 * 1024 * 1024;
/// The most entries all stems of a program hold together.
pub const MAX_STEM_ENTRIES: usize = 1 << 20;
/// The most lines the queue holds.
pub const MAX_QUEUE: usize = 1 << 16;

/// What the interpreter needs from the shell around it.
pub trait Host {
    /// Write a line (SAY), without its line end.
    fn say(&mut self, line: &[u8]);
    /// Run `command` in the environment `env` (its name in upper case).
    /// Returns the return code, or `None` for an environment the host does
    /// not have.
    fn command(&mut self, env: &[u8], command: &[u8]) -> Option<i64>;
    /// A line of terminal input (PULL with an empty queue, PARSE EXTERNAL),
    /// or `None` at its end.
    fn read_line(&mut self) -> Option<Vec<u8>>;
    /// Whether the program should stop (a signal, the shell ending).
    fn interrupted(&self) -> bool;
    /// The local date and time now, with hundredths of a second.
    fn now(&self) -> Option<(LocalTime, u8)>;
    /// An external routine `name`: the source of a REXX program found for
    /// it, and its OS/2 path.
    fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)>;
    /// A variable of the shell's environment (VALUE(name,,'OS2ENVIRONMENT')).
    fn get_env(&self, name: &[u8]) -> Option<Vec<u8>>;
    /// Set (or, with `None`, remove) a variable of the shell's environment.
    fn set_env(&mut self, name: &[u8], value: Option<&[u8]>);
}

/// Why running stopped early: a REXX error (its number, the line it
/// happened on, more detail), or an EXIT inside a routine, which ends the
/// whole program (`exit` holds its value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RexxError {
    pub code: u32,
    pub line: usize,
    pub detail: Option<String>,
    pub exit: Option<Option<Vec<u8>>>,
}

impl RexxError {
    pub fn new(code: u32) -> Self { RexxError { code, line: 0, detail: None, exit: None } }
    pub fn with(code: u32, detail: impl Into<String>) -> Self { RexxError { code, line: 0, detail: Some(detail.into()), exit: None } }
    fn exit(v: Option<Vec<u8>>) -> Self { RexxError { code: 0, line: 0, detail: None, exit: Some(v) } }
}

pub type R<T> = Result<T, RexxError>;

/// The text of REXX error `code`.
pub fn error_message(code: u32) -> &'static str {
    match code {
        3 => "Program is unreadable",
        4 => "Program interrupted",
        5 => "System resources exhausted",
        6 => "Unmatched \"/*\" or quote",
        7 => "WHEN or OTHERWISE expected",
        8 => "Unexpected THEN or ELSE",
        9 => "Unexpected WHEN or OTHERWISE",
        10 => "Unexpected or unmatched END",
        11 => "Control stack full",
        13 => "Invalid character in program",
        14 => "Incomplete DO/SELECT/IF",
        15 => "Invalid hexadecimal or binary string",
        16 => "Label not found",
        17 => "Unexpected PROCEDURE",
        18 => "THEN expected",
        19 => "String or symbol expected",
        20 => "Name expected",
        21 => "Invalid data on end of clause",
        25 => "Invalid sub-keyword found",
        26 => "Invalid whole number",
        27 => "Invalid DO syntax",
        28 => "Invalid LEAVE or ITERATE",
        31 => "Name starts with number or \".\"",
        33 => "Invalid expression result",
        34 => "Logical value not 0 or 1",
        35 => "Invalid expression",
        36 => "Unmatched \"(\" in expression",
        37 => "Unexpected \",\" or \")\"",
        38 => "Invalid template or pattern",
        40 => "Incorrect call to routine",
        41 => "Bad arithmetic conversion",
        42 => "Arithmetic overflow/underflow",
        43 => "Routine not found",
        44 => "Function did not return data",
        45 => "No data specified on function RETURN",
        48 => "Failure in system service",
        49 => "Interpretation error",
        _ => "Error",
    }
}

/// How a program or routine was called (PARSE SOURCE's second word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType { Command, Function, Subroutine }

impl CallType {
    fn word(self) -> &'static str {
        match self { CallType::Command => "COMMAND", CallType::Function => "FUNCTION", CallType::Subroutine => "SUBROUTINE" }
    }
}

/// A compound variable's entries and the stem's default.
#[derive(Debug, Default, Clone)]
struct Stem {
    default: Option<Vec<u8>>,
    /// `None`: dropped (no value, even with a default).
    items: HashMap<Vec<u8>, Option<Vec<u8>>>,
}

/// The variables of one PROCEDURE level (or of the program).
#[derive(Debug, Default)]
struct Scope {
    vars: HashMap<Vec<u8>, Vec<u8>>,
    stems: HashMap<Vec<u8>, Stem>,
    /// Names (simple, `STEM.` or a full compound name) that belong to the
    /// scope at the given index (PROCEDURE EXPOSE).
    exposed: HashMap<Vec<u8>, usize>,
}

/// Settings a routine inherits and that come back on its return.
#[derive(Debug, Clone)]
struct Settings {
    digits: usize,
    fuzz: usize,
    engineering: bool,
    address: Vec<u8>,
    previous_address: Vec<u8>,
}

/// One running routine or program.
struct Activation {
    args: Vec<Option<Vec<u8>>>,
    call_type: CallType,
    /// Whether PROCEDURE may come next (the first instruction of a called
    /// internal routine).
    procedure_allowed: bool,
    /// Whether this activation made a PROCEDURE scope it must pop.
    own_scope: bool,
}

/// A DO or SELECT the run loop is inside.
enum Ctl {
    Group { do_idx: usize },
    Loop { do_idx: usize, var: Option<Target>, var_name: Option<Vec<u8>>, to: Option<Num>, by: Num, left: Option<i64>, spec: Rc<DoSpec> },
    Select { idx: usize, matched: bool },
}

/// How running a piece of program ended.
pub enum Outcome {
    /// Its end was reached.
    Fell,
    Return(Option<Vec<u8>>),
    Exit(Option<Vec<u8>>),
    /// A SIGNAL from inside INTERPRET, to a label of the program around it.
    Signal(Vec<u8>),
}

/// The interpreter's state for one program run.
pub struct Interp<'h> {
    pub host: &'h mut dyn Host,
    scopes: Vec<Scope>,
    pub queue: VecDeque<Vec<u8>>,
    settings: Settings,
    call_depth: usize,
    interpret_depth: usize,
    /// PARSE SOURCE's third word: the program's OS/2 path.
    source_name: Vec<u8>,
    source_lines: Rc<Vec<Vec<u8>>>,
    /// The elapsed-time clock of TIME('E') / TIME('R').
    pub timer_start: Option<std::time::Instant>,
    pub random_state: u64,
    stem_entries: usize,
    /// The arguments of each active routine, the running one last (ARG()).
    pub args_stack: Vec<Vec<Option<Vec<u8>>>>,
}

/// The default environment for commands.
pub const DEFAULT_ENV: &[u8] = b"CMD";

/// PARSE VERSION: the implementation, the language level, the date.
pub const VERSION: &str = concat!("REXX-Warpine_", env!("CARGO_PKG_VERSION"), " 4.00 20 Nov 2026");

/// Run the REXX program `source` (from `name`, its OS/2 path) with `args`.
/// Returns the program's result (`RETURN` / `EXIT` value), or the error
/// that ended it, with its line.
pub fn run_program(host: &mut dyn Host, source: &[u8], name: &[u8], args: Vec<Option<Vec<u8>>>, call_type: CallType)
                   -> Result<Option<Vec<u8>>, RexxError> {
    let mut interp = Interp {
        host,
        scopes: vec![Scope::default()],
        queue: VecDeque::new(),
        settings: Settings { digits: 9, fuzz: 0, engineering: false, address: DEFAULT_ENV.to_vec(), previous_address: DEFAULT_ENV.to_vec() },
        call_depth: 0,
        interpret_depth: 0,
        source_name: name.to_vec(),
        source_lines: Rc::new(source.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l).to_vec()).collect()),
        timer_start: None,
        random_state: 0,
        stem_entries: 0,
        args_stack: Vec::new(),
    };
    match interp.run_external(source, name, args, call_type) {
        Err(RexxError { exit: Some(v), .. }) => Ok(v),
        other => other,
    }
}

/// A program's clauses as instructions (syntax errors with their line).
pub fn compile(source: &[u8]) -> R<Program> {
    let clauses = tokenize(source).map_err(|e| RexxError { code: e.code, line: e.line, detail: None, exit: None })?;
    build(clauses).map_err(|e| RexxError { code: e.code, line: e.line, detail: None, exit: None })
}

impl Interp<'_> {
    /// Run a program (the main one, or an external routine) in its own
    /// variable scope.
    fn run_external(&mut self, source: &[u8], name: &[u8], args: Vec<Option<Vec<u8>>>, call_type: CallType) -> R<Option<Vec<u8>>> {
        let prog = Rc::new(compile(source)?);
        self.args_stack.push(args.clone());
        let mut act = Activation { args, call_type, procedure_allowed: false, own_scope: false };
        let saved_name = std::mem::replace(&mut self.source_name, name.to_vec());
        let outcome = self.run(&prog, 0, &mut act, true);
        self.source_name = saved_name;
        self.args_stack.pop();
        match outcome? {
            Outcome::Fell => Ok(None),
            Outcome::Return(v) | Outcome::Exit(v) => Ok(v),
            Outcome::Signal(label) => Err(RexxError::with(16, String::from_utf8_lossy(&label))),
        }
    }

    pub fn digits(&self) -> usize { self.settings.digits }
    pub fn fuzz(&self) -> usize { self.settings.fuzz }
    pub fn engineering(&self) -> bool { self.settings.engineering }
    pub fn address(&self) -> &[u8] { &self.settings.address }
    pub fn source_line(&self, n: usize) -> Option<&[u8]> { self.source_lines.get(n.checked_sub(1)?).map(Vec::as_slice) }
    pub fn source_line_count(&self) -> usize { self.source_lines.len() }

    // ── Running ─────────────────────────────────────────────────────────

    /// Run `prog` from `start` until its end, RETURN, EXIT or an error. A
    /// SIGNAL goes to a label of `prog`, or, when `own_labels` is false
    /// (INTERPRET), up to the program around it.
    fn run(&mut self, prog: &Rc<Program>, start: usize, act: &mut Activation, own_labels: bool) -> R<Outcome> {
        let mut pc = start;
        let mut ctl: Vec<Ctl> = Vec::new();
        loop {
            let Some(instr) = prog.instrs.get(pc) else { return Ok(Outcome::Fell) };
            if self.host.interrupted() {
                return Err(RexxError { code: 4, line: prog.lines[pc], detail: None, exit: None });
            }
            let line = prog.lines[pc];
            let at = |mut e: RexxError| { if e.line == 0 { e.line = line; } e };
            let first = act.procedure_allowed;
            act.procedure_allowed = false;
            let next = match self.step(prog, pc, instr, act, &mut ctl, first) {
                Ok(Step::Next) => pc + 1,
                Ok(Step::Jump(to)) => to,
                Ok(Step::Done(outcome)) => match outcome {
                    Outcome::Signal(label) if !own_labels => return Ok(Outcome::Signal(label)),
                    Outcome::Signal(label) => match prog.labels.get(&label) {
                        Some(&to) => { ctl.clear(); to }
                        None => return Err(at(RexxError::with(16, String::from_utf8_lossy(&label)))),
                    },
                    o => return Ok(o),
                },
                Err(e) => return Err(at(e)),
            };
            pc = next;
        }
    }

    fn step(&mut self, prog: &Rc<Program>, pc: usize, instr: &Instr, act: &mut Activation, ctl: &mut Vec<Ctl>, first: bool) -> R<Step> {
        match instr {
            Instr::Nop | Instr::Label(_) | Instr::Then | Instr::Trace => Ok(Step::Next),
            Instr::Say(e) => {
                let v = match e { Some(e) => self.eval(e, prog)?, None => Vec::new() };
                self.host.say(&v);
                Ok(Step::Next)
            }
            Instr::Assign(target, e) => {
                let v = self.eval(e, prog)?;
                self.assign(target, v)?;
                Ok(Step::Next)
            }
            Instr::Command(e) => {
                let cmd = self.eval(e, prog)?;
                let env = self.settings.address.clone();
                self.run_command(&env, &cmd);
                Ok(Step::Next)
            }
            Instr::If(cond) => {
                let (then_start, else_start, end) = prog.if_jumps[&pc];
                Ok(Step::Jump(if self.truth(cond, prog)? { then_start } else { else_start.unwrap_or(end) }))
            }
            Instr::Else => Ok(Step::Jump(prog.else_skip[&pc])),
            Instr::Do(spec) => self.start_do(prog, pc, spec, ctl),
            Instr::End(name) => self.end(prog, pc, name.as_deref(), ctl),
            Instr::Select => {
                ctl.push(Ctl::Select { idx: pc, matched: false });
                Ok(Step::Next)
            }
            Instr::When(cond) => {
                let Some(Ctl::Select { idx, matched }) = ctl.last_mut() else { return Err(RexxError::new(9)) };
                let end = prog.end_of[idx];
                if *matched { ctl.pop(); return Ok(Step::Jump(end + 1)); }
                let (start, next) = prog.when_jumps[&pc];
                if self.truth(cond, prog)? {
                    if let Some(Ctl::Select { matched, .. }) = ctl.last_mut() { *matched = true; }
                    return Ok(Step::Jump(start));
                }
                // No WHEN held and no OTHERWISE: Error 7.
                if matches!(prog.instrs[next], Instr::End(_)) { return Err(RexxError::new(7)); }
                Ok(Step::Jump(next))
            }
            Instr::Otherwise => {
                let Some(Ctl::Select { idx, matched }) = ctl.last() else { return Err(RexxError::new(9)) };
                if *matched {
                    let end = prog.end_of[idx];
                    ctl.pop();
                    return Ok(Step::Jump(end + 1));
                }
                Ok(Step::Next)
            }
            Instr::Iterate(name) | Instr::Leave(name) => {
                let leave = matches!(instr, Instr::Leave(_));
                let found = ctl.iter().rposition(|c| match c {
                    Ctl::Loop { var_name, .. } => name.is_none() || var_name.as_deref() == name.as_deref(),
                    _ => false,
                }).ok_or_else(|| RexxError::new(28))?;
                ctl.truncate(found + 1);
                let Some(Ctl::Loop { do_idx, .. }) = ctl.last() else { unreachable!() };
                let end = prog.end_of[do_idx];
                if leave { ctl.pop(); return Ok(Step::Jump(end + 1)); }
                self.next_iteration(prog, ctl, end)
            }
            Instr::Call { name, literal, args } => {
                let mut values = Vec::with_capacity(args.len());
                for a in args { values.push(match a { Some(e) => Some(self.eval(e, prog)?), None => None }); }
                let result = self.call(name, *literal, values, CallType::Subroutine, prog)?;
                match result {
                    Some(v) => self.set_var(b"RESULT", v)?,
                    None => self.drop_var(&Target::Var(b"RESULT".to_vec()))?,
                }
                Ok(Step::Next)
            }
            Instr::Signal(label) => Ok(Step::Done(Outcome::Signal(label.clone()))),
            Instr::SignalValue(e) => {
                let label = self.eval(e, prog)?.to_ascii_uppercase();
                Ok(Step::Done(Outcome::Signal(label)))
            }
            Instr::Return(e) => {
                let v = match e { Some(e) => Some(self.eval(e, prog)?), None => None };
                Ok(Step::Done(Outcome::Return(v)))
            }
            Instr::Exit(e) => {
                let v = match e { Some(e) => Some(self.eval(e, prog)?), None => None };
                Ok(Step::Done(Outcome::Exit(v)))
            }
            Instr::Procedure(names) => {
                if !first { return Err(RexxError::new(17)); }
                self.procedure(names, act)?;
                Ok(Step::Next)
            }
            Instr::Drop(names) => {
                for target in self.resolve_names(names)? { self.drop_var(&target)?; }
                Ok(Step::Next)
            }
            Instr::Interpret(e) => {
                let source = self.eval(e, prog)?;
                match self.interpret(&source, act)? {
                    Outcome::Fell => Ok(Step::Next),
                    o => Ok(Step::Done(o)),
                }
            }
            Instr::Parse { upper, source, templates } => {
                self.parse_instr(*upper, source, templates, act, prog)?;
                Ok(Step::Next)
            }
            Instr::Push(e) | Instr::Queue(e) => {
                let v = match e { Some(e) => self.eval(e, prog)?, None => Vec::new() };
                if self.queue.len() >= MAX_QUEUE { return Err(RexxError::with(5, "the queue is full")); }
                if matches!(instr, Instr::Push(_)) { self.queue.push_front(v) } else { self.queue.push_back(v) }
                Ok(Step::Next)
            }
            Instr::Address { env, command, value } => {
                let s = &mut self.settings;
                match (env, command, value) {
                    (None, None, None) => std::mem::swap(&mut s.address, &mut s.previous_address),
                    (Some(env), Some(cmd), _) => {
                        let cmd = self.eval(cmd, prog)?;
                        let env = env.clone();
                        self.run_command(&env, &cmd);
                    }
                    (Some(env), None, _) => {
                        s.previous_address = std::mem::replace(&mut s.address, env.clone());
                    }
                    (None, _, Some(e)) => {
                        let env = self.eval(e, prog)?;
                        let s = &mut self.settings;
                        s.previous_address = std::mem::replace(&mut s.address, env);
                    }
                    _ => {}
                }
                Ok(Step::Next)
            }
            Instr::NumericDigits(e) => {
                let digits = match e { Some(e) => self.whole_arg(e, prog)?, None => 9 };
                if digits < 1 || digits as usize > MAX_DIGITS || digits as usize <= self.settings.fuzz {
                    return Err(RexxError::with(33, format!("NUMERIC DIGITS {digits}")));
                }
                self.settings.digits = digits as usize;
                Ok(Step::Next)
            }
            Instr::NumericFuzz(e) => {
                let fuzz = match e { Some(e) => self.whole_arg(e, prog)?, None => 0 };
                if fuzz < 0 || fuzz as usize >= self.settings.digits {
                    return Err(RexxError::with(33, format!("NUMERIC FUZZ {fuzz}")));
                }
                self.settings.fuzz = fuzz as usize;
                Ok(Step::Next)
            }
            Instr::NumericForm(e) => {
                let form = match e { Some(e) => self.eval(e, prog)?.to_ascii_uppercase(), None => b"SCIENTIFIC".to_vec() };
                self.settings.engineering = match form.first() {
                    Some(b'E') => true,
                    Some(b'S') => false,
                    _ => return Err(RexxError::with(33, "NUMERIC FORM")),
                };
                Ok(Step::Next)
            }
        }
    }

    /// Run a command; `RC` takes its return code (-3 for an unknown
    /// environment, unverified).
    fn run_command(&mut self, env: &[u8], cmd: &[u8]) {
        let rc = self.host.command(&env.to_ascii_uppercase(), cmd).unwrap_or(-3);
        let _ = self.set_var(b"RC", rc.to_string().into_bytes());
    }

    // ── DO loops ────────────────────────────────────────────────────────

    fn start_do(&mut self, prog: &Rc<Program>, pc: usize, spec: &DoSpec, ctl: &mut Vec<Ctl>) -> R<Step> {
        let end = prog.end_of[&pc];
        if spec.is_simple() {
            ctl.push(Ctl::Group { do_idx: pc });
            return Ok(Step::Next);
        }
        let digits = self.settings.digits;
        let mut var = None;
        let mut var_name = None;
        let (mut to, mut by) = (None, Num::from_i64(1));
        if let Some((target, init)) = &spec.control {
            let v = self.eval(init, prog)?;
            let start = self.number(&v)?.round(digits);
            if let Some(e) = &spec.to { let v = self.eval(e, prog)?; to = Some(self.number(&v)?.round(digits)); }
            if let Some(e) = &spec.by { let v = self.eval(e, prog)?; by = self.number(&v)?.round(digits); }
            self.assign(target, start.format(digits))?;
            var_name = Some(match target {
                Target::Var(n) => n.clone(),
                Target::Compound(Expr::Compound { stem, .. }) => stem.clone(),
                _ => Vec::new(),
            });
            var = Some(target.clone());
        }
        let left = match &spec.count {
            Some(e) => {
                let n = self.whole_arg(e, prog)?;
                if n < 0 { return Err(RexxError::with(26, "a negative repetition count")); }
                Some(n)
            }
            None => None,
        };
        ctl.push(Ctl::Loop { do_idx: pc, var, var_name, to, by, left, spec: Rc::new(spec.clone()) });
        if self.loop_continues(prog, ctl)? { Ok(Step::Next) } else { ctl.pop(); Ok(Step::Jump(end + 1)) }
    }

    /// Whether the innermost loop runs (again): its control variable within
    /// TO, its count not used up, its WHILE true. Counts the iteration.
    fn loop_continues(&mut self, prog: &Rc<Program>, ctl: &mut [Ctl]) -> R<bool> {
        let Some(Ctl::Loop { var, to, by, left, spec, .. }) = ctl.last_mut() else { return Ok(false) };
        if let (Some(target), Some(to)) = (var.as_ref(), to.as_ref()) {
            let v = self.value_of_target(target)?;
            let v = self.number(&v)?;
            let beyond = if by.is_negative() { v.cmp_value(to) == Ordering::Less } else { v.cmp_value(to) == Ordering::Greater };
            if beyond { return Ok(false); }
        }
        if let Some(n) = left {
            if *n == 0 { return Ok(false); }
            *n -= 1;
        }
        if let Some(w) = spec.while_.clone() && !self.truth(&w, prog)? { return Ok(false); }
        Ok(true)
    }

    /// At END (or ITERATE): UNTIL, the control variable's step, and the
    /// tests of a new iteration.
    fn next_iteration(&mut self, prog: &Rc<Program>, ctl: &mut Vec<Ctl>, end: usize) -> R<Step> {
        let Some(Ctl::Loop { do_idx, var, by, spec, .. }) = ctl.last() else { return Err(RexxError::new(10)) };
        let do_idx = *do_idx;
        if let Some(u) = spec.until.clone() && self.truth(&u, prog)? {
            ctl.pop();
            return Ok(Step::Jump(end + 1));
        }
        if let Some(target) = var.clone() {
            let by = by.clone();
            let v = self.number(&self.value_of_target(&target)?)?;
            let next = v.add(&by, self.settings.digits).map_err(arith)?;
            self.assign(&target, next.format(self.settings.digits))?;
        }
        if self.loop_continues(prog, ctl)? { Ok(Step::Jump(do_idx + 1)) } else { ctl.pop(); Ok(Step::Jump(end + 1)) }
    }

    fn end(&mut self, prog: &Rc<Program>, pc: usize, name: Option<&[u8]>, ctl: &mut Vec<Ctl>) -> R<Step> {
        let owner = prog.owner_of_end[&pc];
        match ctl.last() {
            Some(Ctl::Group { do_idx }) if *do_idx == owner => {
                if name.is_some() { return Err(RexxError::new(10)); }
                ctl.pop();
                Ok(Step::Next)
            }
            Some(Ctl::Select { idx, .. }) if *idx == owner => {
                ctl.pop();
                Ok(Step::Next)
            }
            Some(Ctl::Loop { do_idx, var_name, .. }) if *do_idx == owner => {
                if let Some(n) = name && var_name.as_deref() != Some(n) { return Err(RexxError::new(10)); }
                self.next_iteration(prog, ctl, pc)
            }
            _ => Err(RexxError::new(10)),
        }
    }

    // ── Routines ────────────────────────────────────────────────────────

    /// Call routine `name`: an internal label (not for a string name), a
    /// built-in function, or an external REXX program. Returns its result.
    pub fn call(&mut self, name: &[u8], literal: bool, args: Vec<Option<Vec<u8>>>, call_type: CallType, prog: &Rc<Program>) -> R<Option<Vec<u8>>> {
        if self.call_depth >= MAX_CALL_DEPTH { return Err(RexxError::with(11, "too many routines active")); }
        self.call_depth += 1;
        let result = self.call_inner(name, literal, args, call_type, prog);
        self.call_depth -= 1;
        result
    }

    fn call_inner(&mut self, name: &[u8], literal: bool, args: Vec<Option<Vec<u8>>>, call_type: CallType, prog: &Rc<Program>) -> R<Option<Vec<u8>>> {
        if !literal && let Some(&label) = prog.labels.get(name) {
            let saved = self.settings.clone();
            self.args_stack.push(args.clone());
            let mut act = Activation { args, call_type, procedure_allowed: true, own_scope: false };
            let outcome = self.run(prog, label + 1, &mut act, true);
            self.args_stack.pop();
            if act.own_scope { self.scopes.pop(); }
            self.settings = saved;
            return match outcome? {
                Outcome::Return(v) => Ok(v),
                Outcome::Fell => Ok(None),
                Outcome::Exit(v) => Err(RexxError::exit(v)),
                Outcome::Signal(l) => Err(RexxError::with(16, String::from_utf8_lossy(&l))),
            };
        }
        let upper = name.to_ascii_uppercase();
        if let Some(result) = super::builtins::call(self, &upper, &args, prog) {
            return result.map(Some);
        }
        if let Some((source, path)) = self.host.external(&upper) {
            let saved_settings = self.settings.clone();
            let saved_scopes = std::mem::take(&mut self.scopes);
            self.scopes.push(Scope::default());
            let saved_lines = std::mem::replace(&mut self.source_lines,
                Rc::new(source.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l).to_vec()).collect()));
            let result = self.run_external(&source, &path, args, call_type);
            self.source_lines = saved_lines;
            self.scopes = saved_scopes;
            self.settings = saved_settings;
            return result;
        }
        Err(RexxError::with(43, String::from_utf8_lossy(name)))
    }

    /// PROCEDURE [EXPOSE names]: a new variable scope; the exposed names
    /// stay the caller's.
    fn procedure(&mut self, names: &[NameRef], act: &mut Activation) -> R<()> {
        // Indirect names are read in the caller's scope.
        let mut targets = Vec::new();
        for n in names {
            match n {
                NameRef::Name(t) => targets.push(t.clone()),
                NameRef::Indirect(var) => {
                    targets.push(Target::Var(var.clone()));
                    targets.extend(self.names_in(var)?);
                }
            }
        }
        let caller = self.scopes.len() - 1;
        let mut scope = Scope::default();
        for t in targets {
            let key = match &t {
                Target::Var(n) => n.clone(),
                Target::Compound(e) => self.compound_name(e)?,
            };
            scope.exposed.insert(key, caller);
        }
        self.scopes.push(scope);
        act.own_scope = true;
        Ok(())
    }

    /// The variable names listed (blank-separated) in `var`'s value.
    fn names_in(&mut self, var: &[u8]) -> R<Vec<Target>> {
        let list = self.get_var(var)?;
        list.split(|&b| b == b' ').filter(|w| !w.is_empty())
            .map(|w| super::program::target_of(w).map_err(RexxError::new)).collect()
    }

    fn resolve_names(&mut self, names: &[NameRef]) -> R<Vec<Target>> {
        let mut out = Vec::new();
        for n in names {
            match n {
                NameRef::Name(t) => out.push(t.clone()),
                NameRef::Indirect(var) => out.extend(self.names_in(var)?),
            }
        }
        Ok(out)
    }

    /// INTERPRET: run `source` here, with this routine's variables.
    fn interpret(&mut self, source: &[u8], act: &mut Activation) -> R<Outcome> {
        if self.interpret_depth >= MAX_INTERPRET_DEPTH { return Err(RexxError::with(11, "too many INTERPRETs active")); }
        let prog = Rc::new(compile(source).map_err(|e| RexxError { line: 0, ..e })?);
        if !prog.labels.is_empty() { return Err(RexxError::with(49, "labels in INTERPRET")); }
        self.interpret_depth += 1;
        let first = act.procedure_allowed;
        let outcome = self.run(&prog, 0, act, false).map_err(|mut e| { e.line = 0; e });
        act.procedure_allowed = first;
        self.interpret_depth -= 1;
        outcome
    }

    // ── Variables ───────────────────────────────────────────────────────

    /// The scope a name lives in, following EXPOSE links.
    fn owner(&self, key: &[u8], stem: Option<&[u8]>) -> usize {
        let mut s = self.scopes.len() - 1;
        loop {
            let scope = &self.scopes[s];
            match scope.exposed.get(key).or_else(|| stem.and_then(|st| scope.exposed.get(st))) {
                Some(&up) => s = up,
                None => return s,
            }
        }
    }

    /// A simple variable's (or stem's) value: its name when unset.
    pub fn get_var(&self, name: &[u8]) -> R<Vec<u8>> {
        if name.ends_with(b".") {
            let s = self.owner(name, None);
            return Ok(self.scopes[s].stems.get(name).and_then(|st| st.default.clone()).unwrap_or_else(|| name.to_vec()));
        }
        let s = self.owner(name, None);
        Ok(self.scopes[s].vars.get(name).cloned().unwrap_or_else(|| name.to_vec()))
    }

    /// Whether a simple variable or a compound one (by its full name) has
    /// a value.
    pub fn var_exists(&self, name: &[u8]) -> bool {
        match name.iter().position(|&c| c == b'.') {
            Some(dot) if dot + 1 < name.len() => {
                let (stem, tail) = (&name[..=dot], &name[dot + 1..]);
                let s = self.owner(name, Some(stem));
                self.scopes[s].stems.get(stem).is_some_and(|st| match st.items.get(tail) {
                    Some(v) => v.is_some(),
                    None => st.default.is_some(),
                })
            }
            Some(_) => {
                let s = self.owner(name, None);
                self.scopes[s].stems.get(name).is_some_and(|st| st.default.is_some())
            }
            None => self.scopes[self.owner(name, None)].vars.contains_key(name),
        }
    }

    /// Whether a compound symbol (its tail substituted) has a value.
    pub fn compound_exists(&self, e: &Expr) -> R<bool> {
        Ok(self.var_exists(&self.compound_name(e)?))
    }

    pub fn set_var(&mut self, name: &[u8], value: Vec<u8>) -> R<()> {
        if value.len() > MAX_STRING { return Err(RexxError::with(5, "a string is too long")); }
        let s = self.owner(name, None);
        if name.ends_with(b".") {
            let stem = self.scopes[s].stems.entry(name.to_vec()).or_default();
            self.stem_entries -= stem.items.len().min(self.stem_entries);
            *stem = Stem { default: Some(value), items: HashMap::new() };
        } else {
            self.scopes[s].vars.insert(name.to_vec(), value);
        }
        Ok(())
    }

    /// A compound symbol's full name: its stem and its tail with the tail's
    /// variables replaced by their values.
    fn compound_name(&self, e: &Expr) -> R<Vec<u8>> {
        let Expr::Compound { stem, tail } = e else { return Err(RexxError::new(20)) };
        let mut name = stem.clone();
        for (k, part) in tail.iter().enumerate() {
            if k > 0 { name.push(b'.'); }
            match part {
                TailPart::Const(c) => name.extend_from_slice(c),
                TailPart::Var(v) => name.extend(self.get_var(v)?),
            }
        }
        Ok(name)
    }

    fn get_compound(&self, full: &[u8], stem_len: usize) -> Vec<u8> {
        let (stem, tail) = (&full[..stem_len], &full[stem_len..]);
        let s = self.owner(full, Some(stem));
        match self.scopes[s].stems.get(stem) {
            Some(st) => match st.items.get(tail) {
                Some(Some(v)) => v.clone(),
                Some(None) => full.to_vec(),
                None => st.default.clone().unwrap_or_else(|| full.to_vec()),
            },
            None => full.to_vec(),
        }
    }

    /// A compound variable's value, by its full name (VALUE()).
    pub fn get_compound_by_name(&self, full: &[u8]) -> Vec<u8> {
        let dot = full.iter().position(|&c| c == b'.').map_or(full.len(), |d| d + 1);
        self.get_compound(full, dot)
    }

    pub fn set_compound(&mut self, full: &[u8], stem_len: usize, value: Vec<u8>) -> R<()> {
        if value.len() > MAX_STRING { return Err(RexxError::with(5, "a string is too long")); }
        let (stem, tail) = (full[..stem_len].to_vec(), full[stem_len..].to_vec());
        let s = self.owner(full, Some(&stem));
        let entries = &mut self.stem_entries;
        let st = self.scopes[s].stems.entry(stem).or_default();
        if !st.items.contains_key(&tail) {
            if *entries >= MAX_STEM_ENTRIES { return Err(RexxError::with(5, "too many compound variables")); }
            *entries += 1;
        }
        st.items.insert(tail, Some(value));
        Ok(())
    }

    pub fn assign(&mut self, target: &Target, value: Vec<u8>) -> R<()> {
        match target {
            Target::Var(name) => self.set_var(name, value),
            Target::Compound(e) => {
                let full = self.compound_name(e)?;
                let Expr::Compound { stem, .. } = e else { unreachable!() };
                self.set_compound(&full, stem.len(), value)
            }
        }
    }

    fn value_of_target(&self, target: &Target) -> R<Vec<u8>> {
        match target {
            Target::Var(n) => self.get_var(n),
            Target::Compound(e) => {
                let full = self.compound_name(e)?;
                let Expr::Compound { stem, .. } = e else { unreachable!() };
                Ok(self.get_compound(&full, stem.len()))
            }
        }
    }

    /// A symbol's value without a call: a constant, a variable or a
    /// compound variable (PARSE's variable patterns, VALUE()).
    pub fn symbol_value(&self, e: &Expr) -> R<Vec<u8>> {
        match e {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Var(n) => self.get_var(n),
            Expr::Compound { stem, .. } => {
                let full = self.compound_name(e)?;
                Ok(self.get_compound(&full, stem.len()))
            }
            _ => Err(RexxError::new(20)),
        }
    }

    pub fn drop_var(&mut self, target: &Target) -> R<()> {
        match target {
            Target::Var(name) if name.ends_with(b".") => {
                let s = self.owner(name, None);
                if let Some(st) = self.scopes[s].stems.remove(name.as_slice()) {
                    self.stem_entries -= st.items.len().min(self.stem_entries);
                }
            }
            Target::Var(name) => {
                let s = self.owner(name, None);
                self.scopes[s].vars.remove(name.as_slice());
            }
            Target::Compound(e) => {
                let full = self.compound_name(e)?;
                let Expr::Compound { stem, .. } = e else { unreachable!() };
                let s = self.owner(&full, Some(stem));
                let tail = full[stem.len()..].to_vec();
                let entries = &mut self.stem_entries;
                let st = self.scopes[s].stems.entry(stem.clone()).or_default();
                if !st.items.contains_key(&tail) { *entries += 1; }
                st.items.insert(tail, None);
            }
        }
        Ok(())
    }

    // ── Expressions ─────────────────────────────────────────────────────

    pub fn eval(&mut self, e: &Expr, prog: &Rc<Program>) -> R<Vec<u8>> {
        match e {
            Expr::Lit(s) => Ok(s.clone()),
            Expr::Var(name) => self.get_var(name),
            Expr::Compound { stem, .. } => {
                let full = self.compound_name(e)?;
                Ok(self.get_compound(&full, stem.len()))
            }
            Expr::Call { name, literal, args } => {
                let mut values = Vec::with_capacity(args.len());
                for a in args { values.push(match a { Some(e) => Some(self.eval(e, prog)?), None => None }); }
                self.call(name, *literal, values, CallType::Function, prog)?
                    .ok_or_else(|| RexxError::with(44, String::from_utf8_lossy(name)))
            }
            Expr::Unary(op, x) => {
                let v = self.eval(x, prog)?;
                match op {
                    UnOp::Not => Ok(if self.logical(&v)? { b"0".to_vec() } else { b"1".to_vec() }),
                    UnOp::Minus => Ok(self.number(&v)?.negated().round(self.settings.digits).format(self.settings.digits)),
                    UnOp::Plus => Ok(self.number(&v)?.round(self.settings.digits).format(self.settings.digits)),
                }
            }
            Expr::Binary(op, a, b) => {
                // Logical operators still evaluate both sides (REXX has no
                // short circuit).
                let a = self.eval(a, prog)?;
                let b = self.eval(b, prog)?;
                self.binary(*op, &a, &b)
            }
        }
    }

    fn binary(&self, op: BinOp, a: &[u8], b: &[u8]) -> R<Vec<u8>> {
        let digits = self.settings.digits;
        let bool_out = |x: bool| if x { b"1".to_vec() } else { b"0".to_vec() };
        let arith = |f: fn(&Num, &Num, usize) -> Result<Num, ArithError>| -> R<Vec<u8>> {
            let (x, y) = (self.number(a)?, self.number(b)?);
            Ok(self.format_num(&f(&x, &y, digits).map_err(arith)?))
        };
        match op {
            BinOp::Add => arith(Num::add),
            BinOp::Sub => arith(Num::sub),
            BinOp::Mul => arith(Num::mul),
            BinOp::Div => arith(Num::div),
            BinOp::IntDiv => arith(Num::int_div),
            BinOp::Rem => arith(Num::rem),
            BinOp::Pow => arith(Num::pow),
            BinOp::Concat | BinOp::BlankConcat => {
                let len = a.len() + b.len() + 1;
                if len > MAX_STRING { return Err(RexxError::with(5, "a string is too long")); }
                let mut out = Vec::with_capacity(len);
                out.extend_from_slice(a);
                if op == BinOp::BlankConcat { out.push(b' '); }
                out.extend_from_slice(b);
                Ok(out)
            }
            BinOp::Eq => Ok(bool_out(self.compare(a, b) == Ordering::Equal)),
            BinOp::Ne => Ok(bool_out(self.compare(a, b) != Ordering::Equal)),
            BinOp::Gt => Ok(bool_out(self.compare(a, b) == Ordering::Greater)),
            BinOp::Lt => Ok(bool_out(self.compare(a, b) == Ordering::Less)),
            BinOp::Ge => Ok(bool_out(self.compare(a, b) != Ordering::Less)),
            BinOp::Le => Ok(bool_out(self.compare(a, b) != Ordering::Greater)),
            BinOp::StrictEq => Ok(bool_out(a == b)),
            BinOp::StrictNe => Ok(bool_out(a != b)),
            BinOp::StrictGt => Ok(bool_out(a > b)),
            BinOp::StrictLt => Ok(bool_out(a < b)),
            BinOp::StrictGe => Ok(bool_out(a >= b)),
            BinOp::StrictLe => Ok(bool_out(a <= b)),
            BinOp::And => Ok(bool_out(self.logical(a)? & self.logical(b)?)),
            BinOp::Or => Ok(bool_out(self.logical(a)? | self.logical(b)?)),
            BinOp::Xor => Ok(bool_out(self.logical(a)? ^ self.logical(b)?)),
        }
    }

    /// Normal comparison: numerically when both are numbers (NUMERIC
    /// FUZZ digits ignored), else as strings without leading and trailing
    /// blanks, the shorter padded with blanks.
    pub fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        if let (Some(x), Some(y)) = (Num::parse(a), Num::parse(b)) {
            let digits = self.settings.digits - self.settings.fuzz;
            return match x.sub(&y, digits) {
                Ok(d) if d.is_zero() => Ordering::Equal,
                Ok(d) if d.is_negative() => Ordering::Less,
                Ok(_) => Ordering::Greater,
                Err(_) => x.cmp_value(&y),
            };
        }
        let trim = |s: &[u8]| -> Vec<u8> {
            let start = s.iter().position(|&c| c != b' ').unwrap_or(s.len());
            let end = s.iter().rposition(|&c| c != b' ').map_or(start, |e| e + 1);
            s[start..end].to_vec()
        };
        let (mut x, mut y) = (trim(a), trim(b));
        let n = x.len().max(y.len());
        x.resize(n, b' ');
        y.resize(n, b' ');
        x.cmp(&y)
    }

    /// A value as a number (Error 41 otherwise).
    pub fn number(&self, v: &[u8]) -> R<Num> {
        Num::parse(v).ok_or_else(|| RexxError::with(41, String::from_utf8_lossy(v).into_owned()))
    }

    /// A number written by the current NUMERIC settings.
    pub fn format_num(&self, n: &Num) -> Vec<u8> {
        n.format(self.settings.digits)
    }

    /// A value that must be 0 or 1 (Error 34 otherwise).
    fn logical(&self, v: &[u8]) -> R<bool> {
        match v {
            b"0" => Ok(false),
            b"1" => Ok(true),
            _ => Err(RexxError::with(34, String::from_utf8_lossy(v).into_owned())),
        }
    }

    fn truth(&mut self, e: &Expr, prog: &Rc<Program>) -> R<bool> {
        let v = self.eval(e, prog)?;
        self.logical(&v)
    }

    /// An expression that must be a whole number.
    fn whole_arg(&mut self, e: &Expr, prog: &Rc<Program>) -> R<i64> {
        let v = self.eval(e, prog)?;
        self.whole(&v)
    }

    pub fn whole(&self, v: &[u8]) -> R<i64> {
        self.number(v)?.to_whole(self.settings.digits)
            .ok_or_else(|| RexxError::with(26, String::from_utf8_lossy(v).into_owned()))
    }

    // ── PARSE ───────────────────────────────────────────────────────────

    fn parse_instr(&mut self, upper: bool, source: &ParseSource, templates: &[Vec<super::program::TemplateItem>],
                   act: &mut Activation, prog: &Rc<Program>) -> R<()> {
        let templates: Vec<&[super::program::TemplateItem]> = if templates.is_empty() { vec![&[]] } else { templates.iter().map(Vec::as_slice).collect() };
        for (k, template) in templates.iter().enumerate() {
            let string = match source {
                ParseSource::Arg => act.args.get(k).cloned().flatten().unwrap_or_default(),
                ParseSource::Pull => match self.queue.pop_front() {
                    Some(line) => line,
                    None => self.host.read_line().unwrap_or_default(),
                },
                ParseSource::External => self.host.read_line().unwrap_or_default(),
                _ if k > 0 => Vec::new(),
                ParseSource::Var(name) => self.get_var(name)?,
                ParseSource::Value(e) => match e { Some(e) => self.eval(e, prog)?, None => Vec::new() },
                ParseSource::Source => {
                    let mut s = format!("OS/2 {} ", act.call_type.word()).into_bytes();
                    s.extend_from_slice(&self.source_name);
                    s
                }
                ParseSource::Version => VERSION.as_bytes().to_vec(),
            };
            let string = if upper { string.to_ascii_uppercase() } else { string };
            super::template::parse(self, &string, template)?;
        }
        Ok(())
    }
}

enum Step {
    Next,
    Jump(usize),
    Done(Outcome),
}

/// An arithmetic error as a REXX error.
pub fn arith(e: ArithError) -> RexxError {
    match e {
        ArithError::Overflow => RexxError::with(42, "overflow"),
        ArithError::DivideByZero => RexxError::with(42, "division by zero"),
        ArithError::NotWhole => RexxError::with(26, "a whole number was needed"),
    }
}
