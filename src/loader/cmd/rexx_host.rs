// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX scripts in the built-in CMD.EXE (ADR-0083): a `.CMD` whose first
// line starts with `/*` runs in the REXX interpreter (`loader::rexx`) with
// this shell as its host. Its commands (ADDRESS CMD) run here, echoed with
// the prompt while ECHO is on as OS/2's CMD.EXE does (hence the usual
// `'@ECHO OFF'`); SAY writes to the shell's output; PULL reads its input.

use super::*;
use crate::loader::codepage::{cp_decode, cp_encode};
use crate::loader::rexx::interp::{error_message, run_program, CallType, Host};

/// The stack a REXX program runs on: its routines and expressions nest on
/// it, within the interpreter's own limits.
const REXX_STACK: usize = 64 * 1024 * 1024;

/// The largest REXX program read as an external routine.
const MAX_REXX_SOURCE: u64 = 4 * 1024 * 1024;

/// Whether a script is REXX: its first line starts with `/*`, blanks
/// before it allowed (unverified).
pub(super) fn is_rexx(bytes: &[u8]) -> bool {
    let first_line = bytes.split(|&b| b == b'\n').next().unwrap_or_default();
    let start = first_line.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(first_line.len());
    first_line[start..].starts_with(b"/*")
}

/// The shell as the REXX interpreter's host.
struct ShellHost<'a> {
    shell: &'a mut CmdShell,
    codepage: u32,
}

impl Host for ShellHost<'_> {
    fn say(&mut self, line: &[u8]) {
        self.shell.write_out(line);
        self.shell.write_out(b"\r\n");
    }

    fn command(&mut self, env: &[u8], command: &[u8]) -> Option<i64> {
        if env != crate::loader::rexx::interp::DEFAULT_ENV { return None; }
        let text = cp_decode(command, self.codepage);
        let line = text.trim();
        if self.shell.echo_on.get() && !line.starts_with('@') {
            self.shell.write_prompt();
            self.shell.writeln(line);
        }
        // EXIT run as a command ends nothing but the command here.
        let _ = self.shell.execute_line(line);
        Some(self.shell.last_rc.get() as i64)
    }

    fn read_line(&mut self) -> Option<Vec<u8>> {
        match self.shell.read_line() {
            ReadResult::Line(l) => Some(cp_encode(&l, self.codepage)),
            ReadResult::Eof => None,
        }
    }

    fn interrupted(&self) -> bool {
        self.shell.shared.exit_requested.load(Ordering::Relaxed)
            || crate::loader::interrupt::call_interrupted(&self.shell.shared)
    }

    fn now(&self) -> Option<(crate::loader::clock::LocalTime, u8)> {
        let ms = self.shell.shared.clock.now_ms();
        crate::loader::clock::unix_to_local(ms.div_euclid(1000)).map(|t| (t, (ms.rem_euclid(1000) / 10) as u8))
    }

    fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let name = cp_decode(name, self.codepage);
        let (path, script, os2) = self.shell.find_program_with_name(&name)?;
        if !script || std::fs::metadata(&path).ok()?.len() > MAX_REXX_SOURCE { return None; }
        let source = std::fs::read(&path).ok()?;
        is_rexx(&source).then(|| (source, cp_encode(&os2, self.codepage)))
    }

    fn get_env(&self, name: &[u8]) -> Option<Vec<u8>> {
        let name = cp_decode(name, self.codepage);
        self.shell.env.borrow().iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| cp_encode(v, self.codepage))
    }

    fn set_env(&mut self, name: &[u8], value: Option<&[u8]>) {
        let name = cp_decode(name, self.codepage);
        let value = value.map(|v| cp_decode(v, self.codepage)).unwrap_or_default();
        let _ = env_set(&mut self.shell.env.borrow_mut(), &name, &value);
    }
}

impl CmdShell {
    /// Run the REXX program `source` (from `os2_name`) with the argument
    /// string `args`. Its EXIT or RETURN value, when a whole number, is
    /// the return code; an error is reported as OS/2 does, with the
    /// failing line, and its number is the return code (unverified).
    pub(super) fn run_rexx(&mut self, source: &[u8], os2_name: &str, args: &str) -> u32 {
        let codepage = self.shared.active_codepage.load(Ordering::Relaxed);
        let name = cp_encode(os2_name, codepage);
        let arg = cp_encode(args, codepage);
        let result = std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new().name("rexx".into()).stack_size(REXX_STACK).spawn_scoped(scope, || {
                let mut host = ShellHost { shell: self, codepage };
                run_program(&mut host, source, &name, vec![Some(arg)], CallType::Command)
            });
            spawned.map(|h| h.join())
        });
        match result {
            Ok(Ok(Ok(value))) => value.and_then(|v| crate::loader::rexx::number::Num::parse(&v)?.to_whole(9))
                .map_or(0, |n| n as u32),
            Ok(Ok(Err(e))) => {
                if e.line > 0 {
                    let text = source.split(|&b| b == b'\n').nth(e.line - 1).unwrap_or_default();
                    let text = cp_decode(text.strip_suffix(b"\r").unwrap_or(text), codepage);
                    self.writeln(&format!("{:6} +++ {}", e.line, text.trim()));
                }
                let mut msg = format!("REX{:04}: Error {} running {}", e.code, e.code, os2_name);
                if e.line > 0 { msg.push_str(&format!(", line {}", e.line)); }
                msg.push_str(&format!(": {}", error_message(e.code)));
                if let Some(d) = e.detail.as_deref().filter(|d| !d.is_empty()) { msg.push_str(&format!(" ({d})")); }
                self.error(&msg);
                e.code
            }
            _ => {
                self.error("The REXX interpreter could not run.");
                RC_BUILTIN_FAILED
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rexx_scripts_start_with_a_comment() {
        assert!(is_rexx(b"/* REXX */\r\nsay 1"));
        assert!(is_rexx(b"  /**/"));
        assert!(!is_rexx(b"@echo off\r\n/* not */"));
        assert!(!is_rexx(b"\r\n/* a blank first line */"));
        assert!(!is_rexx(b""));
    }
}
