// SPDX-License-Identifier: GPL-3.0-or-later
//
// The interpreter's side of the REXX API (ADR-0085): a host that embeds
// it — an OS/2 program through RexxStart — may have handlers for commands
// (subcommand environments), external functions and system exits. The
// interpreter asks for a handler's call with `Interp::callback`; while the
// handler runs, the host may pass on variable-pool requests the handler
// makes (RexxVariablePool), which the interpreter answers with its own
// variables, and the session queue goes to the host and comes back.

use std::collections::VecDeque;

use super::expr::is_valid_symbol;
use super::interp::{Interp, RexxError, R};
use super::program::{target_of, Target};

/// A handler call the interpreter asks the host for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callback {
    /// A command for environment `env` (a registered subcommand handler).
    Subcom { env: Vec<u8>, command: Vec<u8> },
    /// External function `name` (a registered function handler).
    Function { name: Vec<u8>, args: Vec<Option<Vec<u8>>>, subroutine: bool },
    /// A system exit.
    Exit(ExitCall),
}

/// The system exits the interpreter calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitCall {
    /// RXINI: before the program's first clause.
    Init,
    /// RXTER: after its last.
    Term,
    /// RXSIOSAY: a SAY line.
    Say(Vec<u8>),
    /// RXSIOTRC: a TRACE line.
    TraceLine(Vec<u8>),
    /// RXSIOTRD: a line of terminal input (PULL, PARSE EXTERNAL).
    TerminalRead,
    /// RXHLTTST: whether the program should stop.
    HaltTest,
}

/// How a handler call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackResult {
    /// A command's return code (RC) and whether it failed or erred
    /// (RXSUBCOM_FAILURE / RXSUBCOM_ERROR).
    Command { rc: Vec<u8>, error: bool, failure: bool },
    /// A function's value (`None`: none), or `Err` when it reported a
    /// failure (Error 40).
    Function(Result<Option<Vec<u8>>, ()>),
    /// An exit: handled or not, an error to raise (RXEXIT_RAISE_ERROR),
    /// and what it gave (a line read, the halt flag as "1").
    Exit { handled: bool, raise_error: bool, value: Option<Vec<u8>> },
}

/// What the host gives the interpreter while a handler runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackEvent {
    /// The handler returned.
    Done(CallbackResult),
    /// The handler asked the variable pool (RexxVariablePool).
    Pool(Vec<PoolRequest>),
}

// The variable pool's function codes and result flags (`<rexxsaa.h>`).
pub const RXSHV_SET: u8 = 0;
pub const RXSHV_FETCH: u8 = 1;
pub const RXSHV_DROPV: u8 = 2;
pub const RXSHV_SYSET: u8 = 3;
pub const RXSHV_SYFET: u8 = 4;
pub const RXSHV_SYDRO: u8 = 5;
pub const RXSHV_NEXTV: u8 = 6;
pub const RXSHV_PRIV: u8 = 7;
pub const RXSHV_EXIT: u8 = 8;
pub const RXSHV_OK: u8 = 0x00;
pub const RXSHV_NEWV: u8 = 0x01;
pub const RXSHV_LVAR: u8 = 0x02;
pub const RXSHV_TRUNC: u8 = 0x04;
pub const RXSHV_BADN: u8 = 0x08;
pub const RXSHV_MEMFL: u8 = 0x10;
pub const RXSHV_BADF: u8 = 0x80;

/// One SHVBLOCK's request: its function code, name and (to set) value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolRequest {
    pub code: u8,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// The answer to a [`PoolRequest`]: its `shvret` flags, and the name
/// (NEXTV) and value (FETCH, SYFET, NEXTV, PRIV) to give back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PoolAnswer {
    pub ret: u8,
    pub name: Option<Vec<u8>>,
    pub value: Option<Vec<u8>>,
}

/// The queue name a handler is told and PRIV QUENAME gives.
pub const SESSION_QUEUE: &[u8] = b"SESSION";

impl Interp<'_> {
    /// Make a handler call through the host: `None` when the host has no
    /// handler for it. While it runs, the queue is the host's and the
    /// handler's variable-pool requests are answered.
    pub fn callback(&mut self, cb: Callback) -> Option<CallbackResult> {
        if !self.host.has_handler(&cb) { return None; }
        self.host.queue_to_host(std::mem::take(&mut self.queue));
        self.host.callback_begin(cb);
        let result = loop {
            match self.host.callback_next() {
                CallbackEvent::Done(r) => break r,
                CallbackEvent::Pool(requests) => {
                    let answers = self.variable_pool(requests);
                    self.host.pool_answer(answers);
                }
            }
        };
        self.pool_walk = None;
        self.queue = self.host.queue_from_host();
        Some(result)
    }

    /// A system exit's call: `Some(value)` when it handled the event
    /// (Error 48 when it asked for an error).
    pub fn exit_call(&mut self, call: ExitCall) -> R<Option<Option<Vec<u8>>>> {
        match self.callback(Callback::Exit(call)) {
            Some(CallbackResult::Exit { raise_error: true, .. }) => Err(RexxError::with(48, "a system exit failed")),
            Some(CallbackResult::Exit { handled: true, value, .. }) => Ok(Some(value)),
            _ => Ok(None),
        }
    }

    /// SAY's line: to the RXSIO exit, else the host.
    pub fn say_line(&mut self, line: &[u8]) -> R<()> {
        if self.exit_call(ExitCall::Say(line.to_vec()))?.is_none() { self.host.say(line); }
        Ok(())
    }

    /// A TRACE line: to the RXSIO exit, else the host.
    pub fn trace_line(&mut self, line: &[u8]) {
        if !matches!(self.exit_call(ExitCall::TraceLine(line.to_vec())), Ok(Some(_))) { self.host.trace(line); }
    }

    /// A line of terminal input: from the RXSIO exit, else the host.
    pub fn terminal_line(&mut self) -> R<Option<Vec<u8>>> {
        match self.exit_call(ExitCall::TerminalRead)? {
            Some(value) => Ok(Some(value.unwrap_or_default())),
            None => Ok(self.host.read_line()),
        }
    }

    // ── The variable pool ───────────────────────────────────────────────

    /// Answer a handler's RexxVariablePool requests, in the scope of the
    /// routine that is running.
    pub fn variable_pool(&mut self, requests: Vec<PoolRequest>) -> Vec<PoolAnswer> {
        requests.into_iter().map(|r| self.pool_request(r)).collect()
    }

    fn pool_request(&mut self, r: PoolRequest) -> PoolAnswer {
        let ret = |ret| PoolAnswer { ret, ..Default::default() };
        match r.code {
            RXSHV_SET | RXSHV_SYSET | RXSHV_FETCH | RXSHV_SYFET | RXSHV_DROPV | RXSHV_SYDRO => {
                let symbolic = matches!(r.code, RXSHV_SYSET | RXSHV_SYFET | RXSHV_SYDRO);
                let Some(var) = self.pool_name(&r.name, symbolic) else { return ret(RXSHV_BADN) };
                let new = if self.var_exists(&var.full) { RXSHV_OK } else { RXSHV_NEWV };
                self.pool_walk = None;
                let done = match r.code {
                    RXSHV_SET | RXSHV_SYSET => match var.stem_len {
                        Some(n) => self.set_compound(&var.full, n, r.value),
                        None => self.set_var(&var.full, r.value),
                    },
                    RXSHV_FETCH | RXSHV_SYFET => {
                        let value = match var.stem_len {
                            Some(_) => self.get_compound_by_name(&var.full),
                            None => self.get_var(&var.full).unwrap_or_default(),
                        };
                        return PoolAnswer { ret: new, name: None, value: Some(value) };
                    }
                    _ => match var.stem_len {
                        Some(n) => self.drop_compound(&var.full, n),
                        None => self.drop_var(&Target::Var(var.full)),
                    },
                };
                ret(if done.is_ok() { new } else { RXSHV_MEMFL })
            }
            RXSHV_NEXTV => {
                if self.pool_walk.is_none() { self.pool_walk = Some(self.visible_variables()); }
                match self.pool_walk.as_mut().and_then(VecDeque::pop_front) {
                    Some((name, value)) => PoolAnswer { ret: RXSHV_OK, name: Some(name), value: Some(value) },
                    None => {
                        self.pool_walk = None;
                        ret(RXSHV_LVAR)
                    }
                }
            }
            RXSHV_PRIV => match self.private_value(&r.name) {
                Some(v) => PoolAnswer { ret: RXSHV_OK, name: None, value: Some(v) },
                None => ret(RXSHV_BADN),
            },
            // RXSHV_EXIT sets a function exit's value; no exit here has one.
            _ => ret(RXSHV_BADF),
        }
    }

    /// The variable a pool request names: as it is (direct: an upper-case
    /// symbol, a compound's tail taken literally), or as a symbol in the
    /// program (symbolic: upper-cased, the tail's variables substituted).
    fn pool_name(&self, name: &[u8], symbolic: bool) -> Option<PoolName> {
        if name.is_empty() || name[0].is_ascii_digit() || name[0] == b'.' { return None; }
        let dot = name.iter().position(|&c| c == b'.');
        if symbolic {
            let upper = name.to_ascii_uppercase();
            if !is_valid_symbol(&upper) { return None; }
            return match target_of(&upper).ok()? {
                Target::Var(full) => Some(PoolName { full, stem_len: None }),
                Target::Compound(e) => Some(PoolName { full: self.compound_name(&e).ok()?, stem_len: dot.map(|d| d + 1) }),
            };
        }
        let stem = &name[..dot.map_or(name.len(), |d| d + 1)];
        if !is_valid_symbol(stem) || stem.iter().any(u8::is_ascii_lowercase) { return None; }
        Some(PoolName { full: name.to_vec(), stem_len: dot.filter(|&d| d + 1 < name.len()).map(|d| d + 1) })
    }

    /// PRIV: `PARM` (the argument count), `PARM.n`, `SOURCE`, `VERSION`,
    /// `QUENAME`.
    fn private_value(&self, name: &[u8]) -> Option<Vec<u8>> {
        let name = name.to_ascii_uppercase();
        let args = self.args_stack.last().cloned().unwrap_or_default();
        match name.as_slice() {
            b"PARM" => Some(args.len().to_string().into_bytes()),
            b"SOURCE" => Some(self.parse_source_string()),
            b"VERSION" => Some(super::interp::VERSION.as_bytes().to_vec()),
            b"QUENAME" => Some(SESSION_QUEUE.to_vec()),
            _ => {
                let n: usize = std::str::from_utf8(name.strip_prefix(b"PARM.")?).ok()?.parse().ok()?;
                Some(args.get(n.checked_sub(1)?).cloned().flatten().unwrap_or_default())
            }
        }
    }
}

/// A variable a pool request names: its full name, and for a compound
/// variable the length of its stem.
struct PoolName {
    full: Vec<u8>,
    stem_len: Option<usize>,
}
