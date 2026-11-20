// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX conditions (ADR-0084): ERROR, FAILURE, HALT, NOTREADY, NOVALUE and
// SYNTAX, trapped with `SIGNAL ON` or `CALL ON`. The traps are part of a
// routine's settings: a routine starts with its caller's and they come
// back when it returns. CONDITION() reports the one last trapped.

/// A condition a program can trap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Error,
    Failure,
    Halt,
    NotReady,
    NoValue,
    Syntax,
}

/// How many conditions there are (the size of a trap table).
pub const CONDITIONS: usize = 6;

impl Cond {
    /// The condition named `name` (in upper case).
    pub fn from_name(name: &[u8]) -> Option<Cond> {
        Some(match name {
            b"ERROR" => Cond::Error,
            b"FAILURE" => Cond::Failure,
            b"HALT" => Cond::Halt,
            b"NOTREADY" => Cond::NotReady,
            b"NOVALUE" => Cond::NoValue,
            b"SYNTAX" => Cond::Syntax,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Cond::Error => "ERROR",
            Cond::Failure => "FAILURE",
            Cond::Halt => "HALT",
            Cond::NotReady => "NOTREADY",
            Cond::NoValue => "NOVALUE",
            Cond::Syntax => "SYNTAX",
        }
    }

    /// Its place in a trap table.
    pub fn index(self) -> usize { self as usize }

    /// Whether `CALL ON` may trap it (NOVALUE and SYNTAX only by SIGNAL).
    pub fn callable(self) -> bool { !matches!(self, Cond::NoValue | Cond::Syntax) }
}

/// A condition's trap.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Trap {
    #[default]
    Off,
    /// `SIGNAL ON`: go to the label.
    Signal(Vec<u8>),
    /// `CALL ON`: call the label when the clause ends.
    Call(Vec<u8>),
    /// A `CALL ON` trap whose routine is running: the condition is ignored
    /// until it returns (unverified).
    Delayed(Vec<u8>),
}

/// What CONDITION() reports: the condition last trapped, how, and its
/// description (the command, the variable's name, the stream's name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondInfo {
    pub cond: Cond,
    pub by_call: bool,
    pub description: Vec<u8>,
}
