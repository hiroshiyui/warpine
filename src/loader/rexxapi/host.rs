// SPDX-License-Identifier: GPL-3.0-or-later
//
// The interpreter's host for RexxStart (ADR-0085), on the program's own
// interpreter thread. SAY writes to the program's standard output, TRACE
// to its standard error, PULL reads its standard input (handles 1, 2, 0,
// wherever they lead); ADDRESS CMD runs the command in the built-in shell,
// as DosExecPgm("CMD.EXE") does; files go through the VFS.
//
// A handler call — a subcommand handler, an external function, a system
// exit — is made by the thread that called RexxStart: this side writes
// the call's block (RXSTRINGs and their bytes) into new guest memory,
// sends `ToVcpu::Call`, and reads the results back when the call returns.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use super::{Handler, Session, ToInterp, ToVcpu, RXAUTOBUFLEN, RXHLT, RXHLTTST, RXINI, RXINIEXT, RXSIO,
            RXSIOSAY, RXSIOTRC, RXSIOTRD, RXSUBCOM_ERROR, RXSUBCOM_FAILURE, RXTER, RXTEREXT, SESSION_QUEUE};
use crate::loader::codepage::{cp_decode, cp_encode};
use crate::loader::mutex_ext::MutexExt;
use crate::loader::rexx::embed::{Callback, CallbackEvent, CallbackResult, ExitCall, PoolAnswer};
use crate::loader::rexx::interp::{error_message, run_program, CallType, FileInfo, Host, OpenFor, RexxError, MAX_STRING};
use crate::loader::rexx_vfs::VfsFiles;
use crate::loader::Loader;

/// The stack an interpreter thread runs on (as the shell's, ADR-0083).
pub const REXX_STACK: usize = 64 * 1024 * 1024;
/// The largest block of a handler call (its strings together).
const MAX_BLOCK: usize = 16 * 1024 * 1024 + 64 * 1024;
/// The largest program read as an external routine.
const MAX_ROUTINE: u64 = 4 * 1024 * 1024;
/// The exit handlers' "not handled" and "raise an error" results.
const RXEXIT_NOT_HANDLED: i32 = 1;
const RXEXIT_RAISE_ERROR: i32 = -1;
/// The attribute the console is cleared with.
const ATTR_NORMAL: u8 = 0x07;

/// What RexxStart gives its interpreter thread.
pub struct Setup {
    pub session: Arc<Session>,
    pub to_vcpu: mpsc::Sender<ToVcpu>,
    pub from_vcpu: mpsc::Receiver<ToInterp>,
    /// The initial ADDRESS environment.
    pub environment: Vec<u8>,
    /// The program's environment variables (`NAME=VALUE`).
    pub env: Vec<Vec<u8>>,
    /// The system exits, by exit code.
    pub exits: HashMap<i32, Handler>,
}

/// A handler call under way: its block and how to read its results.
struct Pending {
    block: u32,
    kind: PendingKind,
}

enum PendingKind {
    /// A subcommand handler: its flags and RC string in the block.
    Subcom,
    /// A function: its result string at the block's start.
    Function,
    /// An exit; `retc_at`: where a read line's RXSTRING is; `flags_at`:
    /// where the halt flag is.
    Exit { retc_at: Option<u32>, flags_at: Option<u32> },
    /// Nothing could be called: the call fails.
    Failed,
}

/// The interpreter's host for a RexxStart.
struct ApiHost {
    loader: Loader,
    setup: Setup,
    codepage: u32,
    pending: Option<Pending>,
}

/// Run a program for RexxStart and send its end to the calling thread.
pub fn run(loader: Loader, setup: Setup, source: Vec<u8>, name: Vec<u8>, args: Vec<Option<Vec<u8>>>, call_type: CallType) {
    let codepage = loader.shared.active_codepage.load(Ordering::Relaxed);
    let mut host = ApiHost { loader, setup, codepage, pending: None };
    let outcome = run_program(&mut host, &source, &name, args, call_type);
    let done = match outcome {
        Ok(value) => {
            let retc = value.as_deref().and_then(crate::loader::rexx::number::Num::parse)
                .and_then(|n| n.to_whole(9)).and_then(|n| i16::try_from(n).ok()).unwrap_or(0);
            ToVcpu::Done { rc: 0, retc, result: value }
        }
        Err(e) => {
            host.report(&source, &name, &e);
            ToVcpu::Done { rc: -(e.code as i32), retc: 0, result: None }
        }
    };
    let _ = host.setup.to_vcpu.send(done);
}

/// Bytes laid out for a handler call, with their offsets.
#[derive(Default)]
struct Block {
    bytes: Vec<u8>,
}

impl Block {
    fn at(&self) -> u32 { self.bytes.len() as u32 }
    fn u32(&mut self, v: u32) { self.bytes.extend_from_slice(&v.to_le_bytes()); }
    fn data(&mut self, d: &[u8]) -> u32 {
        let at = self.at();
        self.bytes.extend_from_slice(d);
        self.bytes.push(0);
        while !self.bytes.len().is_multiple_of(4) { self.bytes.push(0); }
        at
    }
    fn set_u32(&mut self, at: u32, v: u32) { self.bytes[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes()); }
}

impl ApiHost {
    fn halted(&self) -> bool {
        self.setup.session.halt.load(Ordering::Acquire) || self.loader.shared.exit_requested.load(Ordering::Relaxed)
    }

    fn files(&self) -> VfsFiles<'_> { VfsFiles { shared: &self.loader.shared, codepage: self.codepage } }

    /// Write to OS/2 handle `fd` of the program.
    fn write_handle(&self, fd: u32, bytes: &[u8]) { let _ = self.loader.write_handle(fd, bytes); }

    /// The handler for a callback, and the exit code / subfunction of an
    /// exit call.
    fn handler_for(&self, cb: &Callback) -> Option<u32> {
        let api = self.loader.shared.rexx_api.lock_or_recover();
        let h = match cb {
            Callback::Subcom { env, .. } => api.subcoms.get(env).cloned(),
            Callback::Function { name, .. } => api.functions.get(name).cloned(),
            Callback::Exit(call) => self.setup.exits.get(&exit_codes(call).0).cloned(),
        };
        drop(api);
        h.and_then(|h| self.loader.rexx_handler_address(&h))
    }

    /// Write `block` into new guest memory; its address.
    fn place(&self, mut block: Block, relocate: &[u32]) -> Option<u32> {
        if block.bytes.len() > MAX_BLOCK { return None; }
        let base = self.loader.alloc_zeroed(block.bytes.len().max(4) as u32)?;
        for &at in relocate {
            let v = u32::from_le_bytes(block.bytes[at as usize..at as usize + 4].try_into().ok()?);
            block.set_u32(at, v + base);
        }
        self.loader.guest_write_bytes(base, &block.bytes)?;
        Some(base)
    }

    /// Lay out and start handler call `cb` of `proc_addr`.
    fn start(&mut self, proc_addr: u32, cb: Callback) -> Option<()> {
        let mut b = Block::default();
        let mut relocate = Vec::new();
        // RXSTRING fields: `len`, then the offset of its bytes (relocated).
        let (args, kind): (Vec<u32>, PendingKind) = match cb {
            Callback::Subcom { command, .. } => {
                // cmd RXSTRING, flags, retstr RXSTRING, retstr's buffer, the command.
                b.u32(command.len() as u32); relocate.push(b.at()); b.u32(0);
                b.u32(0);
                b.u32(RXAUTOBUFLEN); relocate.push(b.at()); b.u32(0);
                let buf = b.data(&[0u8; RXAUTOBUFLEN as usize]);
                b.set_u32(16, buf);
                let cmd = b.data(&command);
                b.set_u32(4, cmd);
                let base = self.place(b, &relocate)?;
                (vec![base, base + 8, base + 12], PendingKind::Subcom)
            }
            Callback::Function { name, args, .. } => {
                // retstr RXSTRING and buffer, the name, the queue name, argv, the arguments.
                b.u32(RXAUTOBUFLEN); relocate.push(b.at()); b.u32(0);
                let buf = b.data(&[0u8; RXAUTOBUFLEN as usize]);
                b.set_u32(4, buf);
                let name_at = b.data(&name);
                let queue_at = b.data(SESSION_QUEUE);
                let argv = b.at();
                for _ in &args { b.u32(0); b.u32(0); }
                for (k, a) in args.iter().enumerate() {
                    let slot = argv + k as u32 * 8;
                    if let Some(a) = a {
                        let at = b.data(a);
                        b.set_u32(slot, a.len() as u32);
                        b.set_u32(slot + 4, at);
                        relocate.push(slot + 4);
                    }
                }
                let base = self.place(b, &relocate)?;
                (vec![base + name_at, args.len() as u32, base + argv, base + queue_at, base], PendingKind::Function)
            }
            Callback::Exit(call) => {
                let (code, sub) = exit_codes(&call);
                match call {
                    ExitCall::Say(line) | ExitCall::TraceLine(line) => {
                        b.u32(line.len() as u32); relocate.push(b.at()); b.u32(0);
                        let at = b.data(&line);
                        b.set_u32(4, at);
                        let base = self.place(b, &relocate)?;
                        (vec![code as u32, sub as u32, base], PendingKind::Exit { retc_at: None, flags_at: None })
                    }
                    ExitCall::TerminalRead => {
                        b.u32(RXAUTOBUFLEN); relocate.push(b.at()); b.u32(0);
                        let buf = b.data(&[0u8; RXAUTOBUFLEN as usize]);
                        b.set_u32(4, buf);
                        let base = self.place(b, &relocate)?;
                        (vec![code as u32, sub as u32, base], PendingKind::Exit { retc_at: Some(base), flags_at: None })
                    }
                    ExitCall::HaltTest => {
                        b.u32(0);
                        let base = self.place(b, &relocate)?;
                        (vec![code as u32, sub as u32, base], PendingKind::Exit { retc_at: None, flags_at: Some(base) })
                    }
                    ExitCall::Init | ExitCall::Term => {
                        b.u32(0);
                        let base = self.place(b, &relocate)?;
                        (vec![code as u32, sub as u32, base], PendingKind::Exit { retc_at: None, flags_at: None })
                    }
                }
            }
        };
        // The block starts at the first argument of a subcommand handler,
        // the last of a function (its retstr), the third of an exit.
        let block = match kind { PendingKind::Subcom => args[0], PendingKind::Function => args[4], _ => args[2] };
        self.pending = Some(Pending { block, kind });
        self.setup.to_vcpu.send(ToVcpu::Call { proc_addr, args }).ok()
    }

    /// An RXSTRING the handler gave back at `at`: its bytes, and freeing
    /// the memory it allocated for them (when not our buffer `ours`).
    fn take_string(&self, at: u32, ours: u32) -> Option<Vec<u8>> {
        let len = self.loader.guest_read::<u32>(at)? as usize;
        let ptr = self.loader.guest_read::<u32>(at + 4)?;
        if ptr == 0 { return None; }
        let bytes = self.loader.guest_read_bytes(ptr, len.min(MAX_STRING));
        if ptr != ours { self.loader.dos_free_mem(ptr); }
        bytes
    }

    /// The results of the handler call that returned `rv`.
    fn finish(&mut self, rv: u32) -> CallbackResult {
        let Some(p) = self.pending.take() else { return failed() };
        let base = p.block;
        let result = match p.kind {
            PendingKind::Subcom => {
                let flags = self.loader.guest_read::<u16>(base + 8).unwrap_or(0);
                let rc = self.take_string(base + 12, base + 20).filter(|r| !r.is_empty()).unwrap_or_else(|| b"0".to_vec());
                CallbackResult::Command { rc, error: flags == RXSUBCOM_ERROR, failure: flags == RXSUBCOM_FAILURE }
            }
            PendingKind::Function => {
                let value = self.take_string(base, base + 8);
                CallbackResult::Function(if rv == 0 { Ok(value) } else { Err(()) })
            }
            PendingKind::Exit { retc_at, flags_at } => {
                let code = rv as i32;
                let value = match (retc_at, flags_at) {
                    (Some(at), _) => self.take_string(at, at + 8),
                    (_, Some(at)) => Some(if self.loader.guest_read::<u32>(at).unwrap_or(0) & 1 != 0 { b"1".to_vec() } else { b"0".to_vec() }),
                    _ => None,
                };
                CallbackResult::Exit { handled: code != RXEXIT_NOT_HANDLED && code != RXEXIT_RAISE_ERROR, raise_error: code == RXEXIT_RAISE_ERROR, value }
            }
            PendingKind::Failed => return failed(),
        };
        if base != 0 { self.loader.shared.mem_mgr.lock_or_recover().free(base); }
        result
    }

    /// Report the error that ended the program on its standard error, as
    /// the shell does.
    fn report(&self, source: &[u8], name: &[u8], e: &RexxError) {
        let mut out = Vec::new();
        if e.line > 0 {
            let text = source.split(|&b| b == b'\n').nth(e.line - 1).unwrap_or_default();
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            out.extend_from_slice(format!("{:6} +++ ", e.line).as_bytes());
            out.extend_from_slice(text.trim_ascii());
            out.extend_from_slice(b"\r\n");
        }
        let mut msg = format!("REX{:04}: Error {} running {}", e.code, e.code, cp_decode(name, self.codepage));
        if e.line > 0 { msg.push_str(&format!(", line {}", e.line)); }
        msg.push_str(&format!(": {}", error_message(e.code)));
        out.extend(cp_encode(&msg, self.codepage));
        out.extend_from_slice(b"\r\n");
        self.write_handle(2, &out);
    }

    fn env_value(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.setup.env.iter().find_map(|e| {
            let eq = e.iter().position(|&c| c == b'=')?;
            e[..eq].eq_ignore_ascii_case(name).then(|| e[eq + 1..].to_vec())
        })
    }
}

/// A failed call: what an unanswered handler gives.
fn failed() -> CallbackResult {
    CallbackResult::Exit { handled: false, raise_error: true, value: None }
}

/// The exit code and subfunction of an exit call.
fn exit_codes(call: &ExitCall) -> (i32, i32) {
    match call {
        ExitCall::Init => (RXINI, RXINIEXT),
        ExitCall::Term => (RXTER, RXTEREXT),
        ExitCall::Say(_) => (RXSIO, RXSIOSAY),
        ExitCall::TraceLine(_) => (RXSIO, RXSIOTRC),
        ExitCall::TerminalRead => (RXSIO, RXSIOTRD),
        ExitCall::HaltTest => (RXHLT, RXHLTTST),
    }
}

impl Host for ApiHost {
    fn say(&mut self, line: &[u8]) {
        self.write_handle(1, &[line, b"\r\n"].concat());
    }

    /// `CMD` runs in the built-in shell; other environments without a
    /// handler are unknown.
    fn command(&mut self, env: &[u8], command: &[u8]) -> Option<i64> {
        if env != b"CMD" { return None; }
        let env: Vec<(String, String)> = self.setup.env.iter().filter_map(|e| {
            let text = cp_decode(e, self.codepage);
            text.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))
        }).collect();
        let line = cp_decode(command, self.codepage);
        Some(crate::loader::cmd::run_shell_command(Arc::clone(&self.loader.shared), env, line.trim()) as i64)
    }

    /// A line of the program's standard input, read a byte at a time so
    /// nothing after it is taken.
    fn read_line(&mut self) -> Option<Vec<u8>> {
        let buf = self.loader.alloc_zeroed(8)?;
        let mut line = Vec::new();
        let mut any = false;
        loop {
            if self.loader.dos_read(0, buf, 1, buf + 4) != 0 { break; }
            if self.loader.guest_read::<u32>(buf + 4) != Some(1) { break; }
            any = true;
            match self.loader.guest_read::<u8>(buf) {
                Some(b'\n') => break,
                Some(b'\r') | None => {}
                Some(c) => if line.len() < MAX_STRING { line.push(c) },
            }
        }
        self.loader.shared.mem_mgr.lock_or_recover().free(buf);
        any.then_some(line)
    }

    fn interrupted(&self) -> bool { self.halted() }

    fn now(&self) -> Option<(crate::loader::clock::LocalTime, u8)> {
        let ms = self.loader.shared.clock.now_ms();
        crate::loader::clock::unix_to_local(ms.div_euclid(1000)).map(|t| (t, (ms.rem_euclid(1000) / 10) as u8))
    }

    /// Another REXX program `NAME.CMD`, in the current directory or one on
    /// the PATH.
    fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let path = self.env_value(b"PATH").unwrap_or_default();
        let mut dirs: Vec<Vec<u8>> = vec![Vec::new()];
        dirs.extend(path.split(|&b| b == b';').map(<[u8]>::trim_ascii).filter(|d| !d.is_empty())
            .map(|d| if d.ends_with(b"\\") { d.to_vec() } else { [d, b"\\"].concat() }));
        for dir in dirs {
            let os2 = [&dir[..], name, b".CMD"].concat();
            let host_path = self.loader.shared.drive_mgr.lock_or_recover().resolve_to_host_path(&cp_decode(&os2, self.codepage));
            let Some(p) = host_path.filter(|p| p.is_file()) else { continue };
            if std::fs::metadata(&p).ok()?.len() > MAX_ROUTINE { return None; }
            let source = std::fs::read(&p).ok()?;
            let first = source.split(|&b| b == b'\n').next().unwrap_or_default();
            if !first.trim_ascii_start().starts_with(b"/*") { return None; }
            return Some((source, self.files().full_name(&os2)?));
        }
        None
    }

    fn get_env(&self, name: &[u8]) -> Option<Vec<u8>> { self.env_value(name) }

    /// The program's copy of its environment changes (not its environment
    /// block, unverified).
    fn set_env(&mut self, name: &[u8], value: Option<&[u8]>) {
        self.setup.env.retain(|e| !e.split(|&c| c == b'=').next().is_some_and(|k| k.eq_ignore_ascii_case(name)));
        if let Some(v) = value { self.setup.env.push([name, b"=", v].concat()); }
    }

    fn write_out(&mut self, bytes: &[u8]) { self.write_handle(1, bytes); }
    fn write_err(&mut self, bytes: &[u8]) { self.write_handle(2, bytes); }
    fn trace(&mut self, line: &[u8]) { self.write_handle(2, &[line, b"\r\n"].concat()); }
    fn clear_screen(&mut self) { self.loader.shared.console_mgr.lock_or_recover().clear_screen(ATTR_NORMAL); }

    fn full_name(&mut self, name: &[u8]) -> Option<Vec<u8>> { self.files().full_name(name) }
    fn file_open(&mut self, name: &[u8], mode: OpenFor) -> Result<u32, u32> { self.files().file_open(name, mode) }
    fn file_read(&mut self, handle: u32, pos: u64, buf: &mut [u8]) -> Result<usize, u32> { self.files().file_read(handle, pos, buf) }
    fn file_write(&mut self, handle: u32, pos: u64, data: &[u8]) -> Result<(), u32> { self.files().file_write(handle, pos, data) }
    fn file_size(&mut self, handle: u32) -> Result<u64, u32> { self.files().file_size(handle) }
    fn file_close(&mut self, handle: u32) { self.files().file_close(handle) }
    fn file_info(&mut self, name: &[u8]) -> Option<FileInfo> { self.files().file_info(name) }
    fn file_find(&mut self, pattern: &[u8], limit: usize) -> Result<Vec<FileInfo>, u32> { self.files().file_find(pattern, limit) }
    fn file_delete(&mut self, name: &[u8]) -> u32 { self.files().file_delete(name) }
    fn dir_make(&mut self, name: &[u8]) -> u32 { self.files().dir_make(name) }
    fn dir_remove(&mut self, name: &[u8]) -> u32 { self.files().dir_remove(name) }
    fn drives(&mut self) -> Vec<u8> { self.files().drives() }

    fn initial_environment(&self) -> Vec<u8> { self.setup.environment.clone() }

    fn has_handler(&mut self, cb: &Callback) -> bool { !self.halted() && self.handler_for(cb).is_some() }

    fn callback_begin(&mut self, cb: Callback) {
        let started = match self.handler_for(&cb) {
            Some(proc_addr) => self.start(proc_addr, cb),
            None => None,
        };
        if started.is_none() { self.pending = Some(Pending { block: 0, kind: PendingKind::Failed }); }
    }

    fn callback_next(&mut self) -> CallbackEvent {
        if matches!(self.pending, Some(Pending { kind: PendingKind::Failed, .. })) {
            self.pending = None;
            return CallbackEvent::Done(failed());
        }
        match self.setup.from_vcpu.recv() {
            Ok(ToInterp::Returned(rv)) => CallbackEvent::Done(self.finish(rv)),
            Ok(ToInterp::Pool(requests)) => CallbackEvent::Pool(requests),
            Err(_) => {
                // The calling thread gave the program up.
                self.setup.session.halt.store(true, Ordering::Release);
                self.pending = None;
                CallbackEvent::Done(failed())
            }
        }
    }

    fn pool_answer(&mut self, answers: Vec<PoolAnswer>) { let _ = self.setup.to_vcpu.send(ToVcpu::PoolAnswers(answers)); }

    fn queue_to_host(&mut self, mut queue: VecDeque<Vec<u8>>) {
        let mut api = self.loader.shared.rexx_api.lock_or_recover();
        let shared = api.queues.entry(SESSION_QUEUE.to_vec()).or_default();
        // Lines others added meanwhile come after the program's.
        queue.append(shared);
        *shared = queue;
    }

    fn queue_from_host(&mut self) -> VecDeque<Vec<u8>> {
        let mut api = self.loader.shared.rexx_api.lock_or_recover();
        api.queues.get_mut(SESSION_QUEUE).map(std::mem::take).unwrap_or_default()
    }

    fn trace_request(&mut self) -> Option<bool> {
        match self.setup.session.trace.swap(0, Ordering::AcqRel) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }
}
