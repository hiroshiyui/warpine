// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX.DLL and REXXAPI.DLL (ADR-0085): the REXX API for OS/2 programs, on
// Warpine's own interpreter (`loader::rexx`).
//
// `RexxStart` runs a program on a host thread of its own (`host.rs`,
// `ApiHost`), with its own `Loader` over the process's shared state. The
// program's handlers — subcommand environments, external functions,
// system exits — are guest code: when the interpreter needs one, its
// thread prepares the arguments in guest memory and asks the thread that
// called RexxStart to make the call. That thread waits in `rexx_wait`
// and makes it as a call chain (ADR-0026, `ChainThen::Rexx`), then waits
// again, until the program ends and RexxStart returns. A handler may call
// `RexxVariablePool`: its requests go to the interpreter, which answers
// them from the variables of the routine that is running.
//
// Registrations, queues and sessions are the process's (`RexxApi`, taken
// alone, never with another `SharedState` lock).

pub mod host;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use log::debug;

use super::mutex_ext::MutexExt;
use super::pm_hook::{CallChain, CallStep, ChainEnd, ChainThen};
use super::rexx::embed::{PoolAnswer, PoolRequest, RXSHV_NEXTV, RXSHV_TRUNC};
use super::rexx::interp::{CallType, MAX_STRING};
use super::{ApiResult, Loader};

// ── Constants (`<rexxsaa.h>`) ────────────────────────────────────────────

pub const RXCOMMAND: u32 = 0;
pub const RXSUBROUTINE: u32 = 1;
pub const RXFUNCTION: u32 = 2;
pub const RXAUTOBUFLEN: u32 = 256;

pub const RXSUBCOM_OK: u32 = 0;
pub const RXSUBCOM_ISREG: u16 = 1;
pub const RXSUBCOM_ERROR: u16 = 1;
pub const RXSUBCOM_FAILURE: u16 = 2;
pub const RXSUBCOM_NOTREG: u32 = 30;
pub const RXSUBCOM_BADTYPE: u32 = 1003;

pub const RXFUNC_OK: u32 = 0;
pub const RXFUNC_DEFINED: u32 = 10;
pub const RXFUNC_NOTREG: u32 = 30;

pub const RXEXIT_OK: u32 = 0;
pub const RXEXIT_ISREG: u16 = 1;
pub const RXEXIT_NOTREG: u32 = 30;
pub const RXEXIT_BADTYPE: u32 = 1003;

pub const RXARI_OK: u32 = 0;
pub const RXARI_NOT_FOUND: u32 = 1;

pub const RXMACRO_OK: u32 = 0;
pub const RXMACRO_NO_STORAGE: u32 = 1;
pub const RXMACRO_NOT_FOUND: u32 = 2;

/// The system exits (`RXSYSEXIT.sysexit_code`) and their subfunctions.
pub const RXENDLST: i32 = 0;
pub const RXSIO: i32 = 5;
pub const RXSIOSAY: i32 = 1;
pub const RXSIOTRC: i32 = 2;
pub const RXSIOTRD: i32 = 3;
pub const RXHLT: i32 = 7;
pub const RXHLTTST: i32 = 2;
pub const RXINI: i32 = 9;
pub const RXINIEXT: i32 = 1;
pub const RXTER: i32 = 10;
pub const RXTEREXT: i32 = 1;
/// The exit codes RexxStart accepts (`RXNOOFEXITS` is one past the last).
pub const RXNOOFEXITS: i32 = 11;

/// The variable pool is not available (no handler of this thread runs).
pub const RXSHV_NOAVL: u32 = 144;

// The queue interface's results (from IBM's description of the REXX
// queue functions; no public header has them, unverified).
pub const RXQUEUE_OK: u32 = 0;
pub const RXQUEUE_STORAGE: u32 = 1;
pub const RXQUEUE_SIZE: u32 = 2;
pub const RXQUEUE_DUP: u32 = 3;
pub const RXQUEUE_BADQNAME: u32 = 5;
pub const RXQUEUE_PRIORITY: u32 = 6;
pub const RXQUEUE_BADWAITFLAG: u32 = 7;
pub const RXQUEUE_EMPTY: u32 = 8;
pub const RXQUEUE_NOTREG: u32 = 9;
pub const RXQUEUE_FIFO: u32 = 0;
pub const RXQUEUE_LIFO: u32 = 1;
pub const RXQUEUE_NOWAIT: u32 = 0;
pub const RXQUEUE_WAIT: u32 = 1;

/// OS/2 errors RexxStart returns for its own arguments (unverified).
const ERROR_INVALID_PARAMETER: u32 = 87;

// ── Limits ───────────────────────────────────────────────────────────────

/// The most arguments RexxStart passes on.
pub const MAX_ARGS: u32 = 1024;
/// The largest program RexxStart reads.
pub const MAX_SOURCE: usize = 4 * 1024 * 1024;
/// The most SHVBLOCKs one RexxVariablePool call follows.
pub const MAX_SHV_BLOCKS: usize = 4096;
/// The most bytes RexxStart's arguments, or one RexxVariablePool call's
/// names and values, take together.
pub const MAX_CALL_BYTES: usize = 64 * 1024 * 1024;
/// The most RXSYSEXIT entries RexxStart reads.
pub const MAX_EXITS: usize = 64;
/// The most handlers of each kind, and queues, a process registers.
pub const MAX_REGISTERED: usize = 1024;
/// The most lines all queues hold together.
pub const MAX_QUEUED_LINES: usize = 1 << 16;
/// How long a thread that must stop (killed, signalled, the process
/// ending) waits for its interpreter to end before it gives it up.
const ABANDON_AFTER: Duration = Duration::from_secs(5);
/// How often a waiting thread looks whether it must stop.
const WAIT_SLICE: Duration = Duration::from_millis(50);
/// The name of the queue every program has.
pub const SESSION_QUEUE: &[u8] = b"SESSION";

// ── The process's REXX state ─────────────────────────────────────────────

/// Where a registered handler is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// In the program (`…Exe`): its address.
    Exe(u32),
    /// In a DLL (`…Dll`): found by name among the loaded modules when it
    /// is called.
    Dll { module: String, proc_name: String },
}

/// A registered handler and its 8-byte user area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    pub entry: Entry,
    pub user: [u8; 8],
}

/// Messages from an interpreter thread to the thread that called
/// RexxStart.
pub enum ToVcpu {
    /// Call a handler: `proc_addr(args…)`.
    Call { proc_addr: u32, args: Vec<u32> },
    /// The answers to a RexxVariablePool request.
    PoolAnswers(Vec<PoolAnswer>),
    /// The program ended: RexxStart's result, `retc`, the result string.
    Done { rc: i32, retc: i16, result: Option<Vec<u8>> },
}

/// Messages from the thread that called RexxStart to its interpreter.
pub enum ToInterp {
    /// The handler returned this.
    Returned(u32),
    /// The handler asked the variable pool.
    Pool(Vec<PoolRequest>),
}

/// A running RexxStart.
pub struct Session {
    pub id: u32,
    /// The OS/2 thread that called RexxStart.
    pub tid: u32,
    /// RexxSetHalt, or the calling thread must stop: the program is halted.
    pub halt: AtomicBool,
    /// RexxSetTrace (1) or RexxResetTrace (2) waiting for the interpreter.
    pub trace: AtomicU8,
    /// A handler call is under way (RexxVariablePool may be used).
    pub in_handler: AtomicBool,
    to_vcpu: Mutex<mpsc::Receiver<ToVcpu>>,
    to_interp: Mutex<mpsc::Sender<ToInterp>>,
    /// RexxStart's `retc` and `result` pointers.
    retc_ptr: u32,
    result_ptr: u32,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The process's registrations, queues and running programs.
#[derive(Default)]
pub struct RexxApi {
    pub subcoms: HashMap<Vec<u8>, Handler>,
    pub functions: HashMap<Vec<u8>, Handler>,
    pub exits: HashMap<Vec<u8>, Handler>,
    pub queues: HashMap<Vec<u8>, VecDeque<Vec<u8>>>,
    next_queue: u32,
    sessions: HashMap<u32, Arc<Session>>,
    next_session: u32,
}

impl RexxApi {
    fn queued(&self) -> usize { self.queues.values().map(VecDeque::len).sum() }
}

/// How a wait for an interpreter ended.
enum Wait {
    /// Make this handler call.
    Chain(CallChain),
    /// RexxStart returns this.
    Return(u32),
}

/// A queue name as RexxCreateQueue and the others take it: upper case,
/// letters, digits and `.!?_` (unverified).
fn queue_name(name: &[u8]) -> Option<Vec<u8>> {
    let upper = name.to_ascii_uppercase();
    let valid = !upper.is_empty() && upper.len() <= 250
        && upper.iter().all(|c| c.is_ascii_alphanumeric() || b".!?_".contains(c));
    valid.then_some(upper)
}

impl Loader {
    fn rexx_api(&self) -> std::sync::MutexGuard<'_, RexxApi> { self.shared.rexx_api.lock_or_recover() }

    /// A C string from the program, as bytes (at most 4 KiB).
    fn rx_cstr(&self, ptr: u32) -> Vec<u8> {
        const MAX_NAME: usize = 4096;
        let mut out = Vec::new();
        if ptr == 0 { return out; }
        while out.len() < MAX_NAME {
            match self.guest_read::<u8>(ptr + out.len() as u32) {
                Some(0) | None => break,
                Some(b) => out.push(b),
            }
        }
        out
    }

    /// An RXSTRING's contents (`None` for a null string), at most
    /// `MAX_STRING` bytes.
    pub(crate) fn rx_string(&self, at: u32) -> Option<Vec<u8>> {
        let len = self.guest_read::<u32>(at)? as usize;
        let ptr = self.guest_read::<u32>(at + 4)?;
        if ptr == 0 { return None; }
        self.guest_read_bytes(ptr, len.min(MAX_STRING))
    }

    /// Give `value` to the program in the RXSTRING at `at`: into its
    /// buffer when it has one large enough (its length as given), else
    /// into new memory the program frees with DosFreeMem.
    fn rx_string_out(&self, at: u32, value: &[u8]) -> bool {
        let (len, ptr) = (self.guest_read::<u32>(at).unwrap_or(0), self.guest_read::<u32>(at + 4).unwrap_or(0));
        let ptr = if ptr != 0 && len as usize >= value.len() {
            ptr
        } else {
            match self.alloc_zeroed((value.len() as u32).max(1)) {
                Some(p) => p,
                None => return false,
            }
        };
        self.guest_write_bytes(ptr, value).is_some()
            && self.guest_write::<u32>(at, value.len() as u32).is_some()
            && self.guest_write::<u32>(at + 4, ptr).is_some()
    }

    // ── RexxStart ───────────────────────────────────────────────────────

    /// RexxStart (REXX.1): run a REXX program, from a file (`procname`,
    /// `.CMD` added when it has no extension) or in storage
    /// (`instore[0]`). Returns 0 when it ran, minus the REXX error number
    /// when an error ended it; `retc` takes its result as a whole number
    /// and `result` its result string.
    pub(crate) fn rexx_start(&self, tid: u32, a: [u32; 10]) -> ApiResult {
        let (argc, argv, procname, instore, envname, calltype, exits, retc, result) =
            (a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8]);
        let call_type = match calltype {
            RXCOMMAND => CallType::Command,
            RXSUBROUTINE => CallType::Subroutine,
            RXFUNCTION => CallType::Function,
            _ => return ApiResult::Normal(ERROR_INVALID_PARAMETER),
        };
        if argc > MAX_ARGS || (call_type == CallType::Command && argc > 1) || (argc > 0 && argv == 0) {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }
        let args: Vec<Option<Vec<u8>>> = (0..argc).map(|k| self.rx_string(argv + k * 8)).collect();
        if args.iter().flatten().map(Vec::len).sum::<usize>() > MAX_CALL_BYTES {
            return ApiResult::Normal(ERROR_INVALID_PARAMETER);
        }
        let name = self.rx_cstr(procname);
        // The program: in storage, or a file.
        let (source, source_name) = match instore {
            0 => match self.rexx_program_file(&name) {
                Some(found) => found,
                None => return ApiResult::Normal((-3i32) as u32),
            },
            _ => match self.rx_string(instore) {
                Some(src) if src.len() <= MAX_SOURCE => (src, name.clone()),
                // A tokenized image only, or the macro space: not here.
                _ => return ApiResult::Normal((-3i32) as u32),
            },
        };
        // The system exits the caller named, from the registered ones.
        let mut exit_handlers = HashMap::new();
        if exits != 0 {
            for k in 0..MAX_EXITS as u32 {
                let at = exits + k * 8;
                let code = self.guest_read::<u32>(at + 4).unwrap_or(0) as i32;
                if code == RXENDLST { break; }
                if !(1..RXNOOFEXITS).contains(&code) { return ApiResult::Normal(RXEXIT_BADTYPE); }
                let exit_name = self.rx_cstr(self.guest_read::<u32>(at).unwrap_or(0)).to_ascii_uppercase();
                let Some(h) = self.rexx_api().exits.get(&exit_name).cloned() else {
                    return ApiResult::Normal(RXEXIT_NOTREG);
                };
                exit_handlers.insert(code, h);
            }
        }
        let environment = if envname != 0 { self.rx_cstr(envname).to_ascii_uppercase() } else { b"CMD".to_vec() };
        let env_ptr = self.guest_read::<u32>(super::constants::PIB_BASE + 0x10).unwrap_or(0);
        let env = if env_ptr != 0 { self.read_env_block(env_ptr) } else { Vec::new() };

        let (to_vcpu_tx, to_vcpu_rx) = mpsc::channel();
        let (to_interp_tx, to_interp_rx) = mpsc::channel();
        let session = {
            let mut api = self.rexx_api();
            api.next_session += 1;
            let session = Arc::new(Session {
                id: api.next_session, tid, halt: AtomicBool::new(false), trace: AtomicU8::new(0),
                in_handler: AtomicBool::new(false), to_vcpu: Mutex::new(to_vcpu_rx), to_interp: Mutex::new(to_interp_tx),
                retc_ptr: retc, result_ptr: result, thread: Mutex::new(None),
            });
            api.sessions.insert(session.id, Arc::clone(&session));
            session
        };
        let loader = Loader { vm: Arc::clone(&self.vm), shared: Arc::clone(&self.shared) };
        let setup = host::Setup {
            session: Arc::clone(&session), to_vcpu: to_vcpu_tx, from_vcpu: to_interp_rx, environment, env,
            exits: exit_handlers,
        };
        let spawned = std::thread::Builder::new().name("rexx".into()).stack_size(host::REXX_STACK).spawn(move || {
            host::run(loader, setup, source, source_name, args, call_type);
        });
        match spawned {
            Ok(handle) => *session.thread.lock_or_recover() = Some(handle),
            Err(_) => {
                self.rexx_api().sessions.remove(&session.id);
                return ApiResult::Normal((-5i32) as u32);
            }
        }
        debug!("  RexxStart('{}'): session {}", String::from_utf8_lossy(&name), session.id);
        match self.rexx_wait(&session) {
            Wait::Chain(chain) => self.start_chain(chain),
            Wait::Return(v) => ApiResult::Normal(v),
        }
    }

    /// The program file `name` names: as given, with `.CMD` when it has
    /// no extension, in the current directory or one on the PATH (as the
    /// shell finds scripts; unverified). Its text and full OS/2 name.
    fn rexx_program_file(&self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let name = super::codepage::cp_decode(name, cp);
        let file = name.rsplit(['\\', '/', ':']).next().unwrap_or(&name);
        let candidate = if file.contains('.') { name.clone() } else { format!("{name}.CMD") };
        let mut dirs = vec![String::new()];
        if !name.contains(['\\', '/', ':']) {
            let path = self.rexx_env_value("PATH");
            dirs.extend(path.split(';').map(str::trim).filter(|d| !d.is_empty())
                .map(|d| if d.ends_with(['\\', '/']) { d.to_string() } else { format!("{d}\\") }));
        }
        let files = super::rexx_vfs::VfsFiles { shared: &self.shared, codepage: cp };
        for dir in dirs {
            let os2 = format!("{dir}{candidate}");
            let host_path = self.shared.drive_mgr.lock_or_recover().resolve_to_host_path(&os2);
            let Some(host_path) = host_path.filter(|p| p.is_file()) else { continue };
            if std::fs::metadata(&host_path).ok()?.len() > MAX_SOURCE as u64 { return None; }
            let source = std::fs::read(&host_path).ok()?;
            let full = files.full_name(&super::codepage::cp_encode(&os2, cp))?;
            return Some((source, full));
        }
        None
    }

    /// An environment variable of the program (its environment block).
    fn rexx_env_value(&self, name: &str) -> String {
        let env_ptr = self.guest_read::<u32>(super::constants::PIB_BASE + 0x10).unwrap_or(0);
        if env_ptr == 0 { return String::new(); }
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        self.read_env_block(env_ptr).iter().map(|e| super::codepage::cp_decode(e, cp))
            .find_map(|e| e.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string()))
            .unwrap_or_default()
    }

    /// Wait for session `session`'s interpreter: a handler to call, or
    /// its end. A thread that must stop halts the program, and gives it
    /// up after `ABANDON_AFTER`.
    fn rexx_wait(&self, session: &Arc<Session>) -> Wait {
        let mut stopping: Option<Instant> = None;
        loop {
            let msg = session.to_vcpu.lock_or_recover().recv_timeout(WAIT_SLICE);
            match msg {
                Ok(ToVcpu::Call { proc_addr, args }) => {
                    session.in_handler.store(true, Ordering::Release);
                    let step = CallStep::hook(proc_addr, args, false);
                    return Wait::Chain(CallChain::new(session.tid, vec![step], false, ChainThen::Rexx { session: session.id }));
                }
                Ok(ToVcpu::Done { rc, retc, result }) => {
                    if session.retc_ptr != 0 { self.guest_write::<i16>(session.retc_ptr, retc); }
                    if session.result_ptr != 0 {
                        match result {
                            Some(v) => { self.rx_string_out(session.result_ptr, &v); }
                            None => { self.guest_write::<u32>(session.result_ptr, 0); }
                        }
                    }
                    self.rexx_end_session(session, true);
                    return Wait::Return(rc as u32);
                }
                Ok(ToVcpu::PoolAnswers(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.interrupted() || self.shutting_down() {
                        session.halt.store(true, Ordering::Release);
                        let since = *stopping.get_or_insert_with(Instant::now);
                        if since.elapsed() >= ABANDON_AFTER {
                            self.rexx_end_session(session, false);
                            return Wait::Return((-4i32) as u32);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.rexx_end_session(session, false);
                    return Wait::Return((-48i32) as u32);
                }
            }
        }
    }

    /// Forget a session; `join` its thread when it ended (else it is left
    /// to end on its own: its messages go nowhere).
    fn rexx_end_session(&self, session: &Arc<Session>, join: bool) {
        self.rexx_api().sessions.remove(&session.id);
        let handle = session.thread.lock_or_recover().take();
        if join && let Some(h) = handle { let _ = h.join(); }
    }

    /// A handler call of session `id` returned `result` (the end of its
    /// `ChainThen::Rexx` chain): tell the interpreter and wait again.
    pub(crate) fn rexx_handler_returned(&self, id: u32, result: u32) -> ChainEnd {
        let Some(session) = self.rexx_api().sessions.get(&id).cloned() else { return ChainEnd::Return((-48i32) as u32) };
        session.in_handler.store(false, Ordering::Release);
        let _ = session.to_interp.lock_or_recover().send(ToInterp::Returned(result));
        match self.rexx_wait(&session) {
            Wait::Chain(chain) => ChainEnd::Next(Box::new(chain)),
            Wait::Return(v) => ChainEnd::Return(v),
        }
    }

    /// The innermost session of thread `tid` whose handler runs.
    fn rexx_handler_session(&self, tid: u32) -> Option<Arc<Session>> {
        self.rexx_api().sessions.values()
            .filter(|s| s.tid == tid && s.in_handler.load(Ordering::Acquire))
            .max_by_key(|s| s.id).cloned()
    }

    // ── RexxVariablePool ────────────────────────────────────────────────

    /// RexxVariablePool (REXX.2): a handler reads and changes the running
    /// program's variables through a chain of SHVBLOCKs. Returns the
    /// blocks' `shvret` flags together, or RXSHV_NOAVL when no handler of
    /// this thread runs.
    pub fn rexx_variable_pool(&self, tid: u32, pshv: u32) -> u32 {
        let Some(session) = self.rexx_handler_session(tid) else { return RXSHV_NOAVL };
        let mut blocks = Vec::new();
        let mut requests = Vec::new();
        let mut at = pshv;
        let mut bytes = 0usize;
        while at != 0 && blocks.len() < MAX_SHV_BLOCKS && bytes <= MAX_CALL_BYTES {
            let code = self.guest_read::<u8>(at + 28).unwrap_or(u8::MAX);
            let name = self.rx_string(at + 4).unwrap_or_default();
            // Only a set's value is read.
            let value = if matches!(code, super::rexx::embed::RXSHV_SET | super::rexx::embed::RXSHV_SYSET) {
                self.rx_string(at + 12).unwrap_or_default()
            } else {
                Vec::new()
            };
            bytes += name.len() + value.len();
            requests.push(PoolRequest { code, name, value });
            blocks.push(at);
            at = self.guest_read::<u32>(at).unwrap_or(0);
        }
        if session.to_interp.lock_or_recover().send(ToInterp::Pool(requests)).is_err() { return RXSHV_NOAVL; }
        let answers = loop {
            match session.to_vcpu.lock_or_recover().recv_timeout(WAIT_SLICE) {
                Ok(ToVcpu::PoolAnswers(a)) => break a,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return RXSHV_NOAVL,
            }
        };
        let mut all = 0u32;
        for (block, answer) in blocks.into_iter().zip(answers) {
            let mut ret = answer.ret;
            if let Some(v) = &answer.value { ret |= self.shv_out(block + 12, block + 24, v); }
            if let Some(n) = &answer.name && self.guest_read::<u8>(block + 28) == Some(RXSHV_NEXTV) {
                ret |= self.shv_out(block + 4, block + 20, n);
            }
            self.guest_write::<u8>(block + 29, ret);
            all |= ret as u32;
        }
        all
    }

    /// A value given back in an SHVBLOCK's RXSTRING at `at`, whose buffer
    /// is `*len_at` long: new memory when it has none (unverified),
    /// truncated (RXSHV_TRUNC) when it is too short.
    fn shv_out(&self, at: u32, len_at: u32, value: &[u8]) -> u8 {
        let ptr = self.guest_read::<u32>(at + 4).unwrap_or(0);
        if ptr == 0 {
            let Some(p) = self.alloc_zeroed((value.len() as u32).max(1)) else { return super::rexx::embed::RXSHV_MEMFL };
            self.guest_write_bytes(p, value);
            self.guest_write::<u32>(at, value.len() as u32);
            self.guest_write::<u32>(at + 4, p);
            self.guest_write::<u32>(len_at, value.len() as u32);
            return 0;
        }
        let room = self.guest_read::<u32>(len_at).unwrap_or(0) as usize;
        let n = value.len().min(room);
        self.guest_write_bytes(ptr, &value[..n]);
        self.guest_write::<u32>(at, n as u32);
        if n < value.len() { RXSHV_TRUNC } else { 0 }
    }

    // ── Halt and trace ──────────────────────────────────────────────────

    /// RexxSetHalt / RexxSetTrace / RexxResetTrace (REXX.3–5): act on the
    /// programs thread `tid` (0: every thread) of this process runs.
    pub fn rexx_signal(&self, pid: u32, tid: u32, what: RexxSignal) -> u32 {
        let own_pid = self.guest_read::<u32>(super::constants::PIB_BASE).unwrap_or(0);
        if pid != own_pid { return RXARI_NOT_FOUND; }
        let api = self.rexx_api();
        let mut found = false;
        for s in api.sessions.values().filter(|s| tid == 0 || s.tid == tid) {
            match what {
                RexxSignal::Halt => s.halt.store(true, Ordering::Release),
                RexxSignal::TraceOn => s.trace.store(1, Ordering::Release),
                RexxSignal::TraceOff => s.trace.store(2, Ordering::Release),
            }
            found = true;
        }
        if found { RXARI_OK } else { RXARI_NOT_FOUND }
    }

    // ── Registration (REXXAPI) ──────────────────────────────────────────

    /// Register a handler of `kind` named `name` (upper-cased). Returns
    /// `ok`, or `dup` when one of the name is there (unverified).
    fn rexx_register(&self, kind: Kind, name: u32, entry: Entry, user: u32, ok: u32, dup: u32) -> u32 {
        let name = self.rx_cstr(name).to_ascii_uppercase();
        if name.is_empty() { return dup; }
        let mut area = [0u8; 8];
        if user != 0 && let Some(b) = self.guest_read_bytes(user, 8) { area.copy_from_slice(&b); }
        let mut api = self.rexx_api();
        let table = kind.table(&mut api);
        if table.contains_key(&name) || table.len() >= MAX_REGISTERED { return dup; }
        table.insert(name, Handler { entry, user: area });
        ok
    }

    /// RexxRegisterSubcomExe / ExitExe / FunctionExe.
    pub fn rexx_register_exe(&self, kind: Kind, name: u32, proc_addr: u32, user: u32) -> u32 {
        let (ok, dup) = kind.codes();
        if proc_addr == 0 { return kind.bad(); }
        self.rexx_register(kind, name, Entry::Exe(proc_addr), user, ok, dup)
    }

    /// RexxRegisterSubcomDll / ExitDll / FunctionDll: the handler is
    /// looked up by name among the loaded modules when it is called.
    pub fn rexx_register_dll(&self, kind: Kind, name: u32, module: u32, proc_name: u32, user: u32) -> u32 {
        let (ok, dup) = kind.codes();
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);
        let module = super::codepage::cp_decode(&self.rx_cstr(module), cp);
        let proc_name = super::codepage::cp_decode(&self.rx_cstr(proc_name), cp);
        if module.is_empty() || proc_name.is_empty() { return kind.bad(); }
        self.rexx_register(kind, name, Entry::Dll { module, proc_name }, user, ok, dup)
    }

    /// RexxQuerySubcom / RexxQueryExit (`flag`, `user`), RexxQueryFunction.
    pub fn rexx_query(&self, kind: Kind, name: u32, flag: u32, user: u32) -> u32 {
        let name = self.rx_cstr(name).to_ascii_uppercase();
        let found = kind.table(&mut self.rexx_api()).get(&name).cloned();
        if flag != 0 { self.guest_write::<u16>(flag, u16::from(found.is_some())); }
        match found {
            Some(h) => {
                if user != 0 { self.guest_write_bytes(user, &h.user); }
                kind.codes().0
            }
            None => kind.not_registered(),
        }
    }

    /// RexxDeregisterSubcom / Exit / Function.
    pub fn rexx_deregister(&self, kind: Kind, name: u32) -> u32 {
        let name = self.rx_cstr(name).to_ascii_uppercase();
        match kind.table(&mut self.rexx_api()).remove(&name) {
            Some(_) => kind.codes().0,
            None => kind.not_registered(),
        }
    }

    /// Where handler `h` is: its address, a DLL's export found by name.
    pub(crate) fn rexx_handler_address(&self, h: &Handler) -> Option<u32> {
        match &h.entry {
            Entry::Exe(addr) => Some(*addr),
            Entry::Dll { module, proc_name } => {
                let dlls = self.shared.dll_mgr.lock_or_recover();
                let dll = dlls.find_by_name(module)?;
                if dll.is_builtin { return None; }
                dll.exports_by_name.get(&proc_name.to_ascii_uppercase()).copied()
            }
        }
    }

    // ── Queues (REXXAPI.1–5) ────────────────────────────────────────────

    /// RexxCreateQueue(buffer, buflen, requested, dupflag): a new queue,
    /// named as requested or, when that name is taken (dupflag 1) or none
    /// was asked for, by Warpine; its name goes to `buffer`.
    pub fn rexx_create_queue(&self, buffer: u32, buflen: u32, requested: u32, dupflag: u32) -> u32 {
        let wanted = if requested != 0 { Some(self.rx_cstr(requested)) } else { None };
        let wanted = match wanted {
            Some(w) => match queue_name(&w) { Some(n) => Some(n), None => return RXQUEUE_BADQNAME },
            None => None,
        };
        let mut api = self.rexx_api();
        if api.queues.len() >= MAX_REGISTERED { return RXQUEUE_STORAGE; }
        let mut dup = false;
        let name = match wanted {
            Some(n) if !api.queues.contains_key(&n) && n != SESSION_QUEUE => n,
            other => {
                dup = other.is_some();
                loop {
                    api.next_queue += 1;
                    let n = format!("Q{}", api.next_queue).into_bytes();
                    if !api.queues.contains_key(&n) { break n; }
                }
            }
        };
        if (name.len() + 1) as u32 > buflen { return RXQUEUE_SIZE; }
        api.queues.insert(name.clone(), VecDeque::new());
        drop(api);
        self.guest_write_bytes(buffer, &name);
        self.guest_write::<u8>(buffer + name.len() as u32, 0);
        if dupflag != 0 { self.guest_write::<u32>(dupflag, u32::from(dup)); }
        if dup { RXQUEUE_DUP } else { RXQUEUE_OK }
    }

    /// RexxDeleteQueue(name). The session queue cannot be deleted.
    pub fn rexx_delete_queue(&self, name: u32) -> u32 {
        let Some(name) = queue_name(&self.rx_cstr(name)) else { return RXQUEUE_BADQNAME };
        if name == SESSION_QUEUE { return RXQUEUE_BADQNAME; }
        if self.rexx_api().queues.remove(&name).is_some() { RXQUEUE_OK } else { RXQUEUE_NOTREG }
    }

    /// RexxQueryQueue(name, count).
    pub fn rexx_query_queue(&self, name: u32, count: u32) -> u32 {
        let Some(name) = queue_name(&self.rx_cstr(name)) else { return RXQUEUE_BADQNAME };
        let api = self.rexx_api();
        let n = match api.queues.get(&name) {
            Some(q) => q.len(),
            None if name == SESSION_QUEUE => 0,
            None => return RXQUEUE_NOTREG,
        };
        drop(api);
        if count != 0 { self.guest_write::<u32>(count, n as u32); }
        RXQUEUE_OK
    }

    /// RexxAddQueue(name, data, flag): a line, at the end (FIFO) or the
    /// front (LIFO).
    pub fn rexx_add_queue(&self, name: u32, data: u32, flag: u32) -> u32 {
        let Some(name) = queue_name(&self.rx_cstr(name)) else { return RXQUEUE_BADQNAME };
        if flag != RXQUEUE_FIFO && flag != RXQUEUE_LIFO { return RXQUEUE_PRIORITY; }
        let line = self.rx_string(data).unwrap_or_default();
        let mut api = self.rexx_api();
        if api.queued() >= MAX_QUEUED_LINES { return RXQUEUE_STORAGE; }
        let queue = match api.queues.get_mut(&name) {
            Some(q) => q,
            None if name == SESSION_QUEUE => api.queues.entry(name).or_default(),
            None => return RXQUEUE_NOTREG,
        };
        if flag == RXQUEUE_LIFO { queue.push_front(line) } else { queue.push_back(line) }
        RXQUEUE_OK
    }

    /// RexxPullQueue(name, data, datetime, wait): the first line, waiting
    /// for one with RXQUEUE_WAIT; `datetime` takes the time it is pulled
    /// (unverified).
    pub fn rexx_pull_queue(&self, name: u32, data: u32, datetime: u32, wait: u32) -> u32 {
        let Some(name) = queue_name(&self.rx_cstr(name)) else { return RXQUEUE_BADQNAME };
        if wait != RXQUEUE_NOWAIT && wait != RXQUEUE_WAIT { return RXQUEUE_BADWAITFLAG; }
        let line = loop {
            let mut api = self.rexx_api();
            match api.queues.get_mut(&name) {
                Some(q) => if let Some(line) = q.pop_front() { break line },
                None if name == SESSION_QUEUE => {}
                None => return RXQUEUE_NOTREG,
            }
            drop(api);
            if wait == RXQUEUE_NOWAIT { return RXQUEUE_EMPTY; }
            if self.interrupted() || self.shutting_down() { return super::constants::ERROR_INTERRUPT; }
            std::thread::sleep(WAIT_SLICE);
        };
        if !self.rx_string_out(data, &line) { return RXQUEUE_STORAGE; }
        if datetime != 0 { self.dos_get_date_time(datetime); }
        RXQUEUE_OK
    }

    /// The macro space (REXXAPI.22–28) is not there: nothing is found
    /// and nothing is stored (unverified).
    pub fn rexx_macro_space(&self, name: &str) -> u32 {
        debug!("  [STUB] {name}: no macro space");
        match name {
            "RexxClearMacroSpace" => RXMACRO_OK,
            "RexxAddMacro" | "RexxLoadMacroSpace" => RXMACRO_NO_STORAGE,
            _ => RXMACRO_NOT_FOUND,
        }
    }
}

/// What RexxSetHalt / RexxSetTrace / RexxResetTrace ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RexxSignal { Halt, TraceOn, TraceOff }

/// The kinds of handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Subcom, Function, Exit }

impl Kind {
    fn table(self, api: &mut RexxApi) -> &mut HashMap<Vec<u8>, Handler> {
        match self { Kind::Subcom => &mut api.subcoms, Kind::Function => &mut api.functions, Kind::Exit => &mut api.exits }
    }

    /// Its OK code and its "already registered" one.
    fn codes(self) -> (u32, u32) {
        match self {
            Kind::Subcom => (RXSUBCOM_OK, RXSUBCOM_NOTREG),
            Kind::Function => (RXFUNC_OK, RXFUNC_DEFINED),
            Kind::Exit => (RXEXIT_OK, RXEXIT_NOTREG),
        }
    }

    fn not_registered(self) -> u32 {
        match self { Kind::Subcom => RXSUBCOM_NOTREG, Kind::Function => RXFUNC_NOTREG, Kind::Exit => RXEXIT_NOTREG }
    }

    /// The code for a bad entry point or name.
    fn bad(self) -> u32 {
        match self { Kind::Subcom => RXSUBCOM_BADTYPE, Kind::Function => RXFUNC_NOTREG, Kind::Exit => RXEXIT_BADTYPE }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock loader with `len` bytes of guest memory for a test.
    fn with_memory(len: u32) -> (Loader, u32) {
        let l = Loader::new_mock();
        let buf = l.alloc_zeroed(len).expect("guest memory");
        (l, buf)
    }

    #[test]
    fn handlers_register_once_and_are_queried() {
        let (l, buf) = with_memory(4096);
        l.guest_write_bytes(buf, b"TestEnv\0");
        l.guest_write_bytes(buf + 64, b"USERAREA");
        assert_eq!(l.rexx_register_exe(Kind::Subcom, buf, 0x1234, buf + 64), RXSUBCOM_OK);
        assert_eq!(l.rexx_register_exe(Kind::Subcom, buf, 0x5678, 0), RXSUBCOM_NOTREG, "the name is taken");
        assert_eq!(l.rexx_register_exe(Kind::Function, buf, 0, 0), RXFUNC_NOTREG, "no entry point");
        assert_eq!(l.rexx_query(Kind::Subcom, buf, buf + 100, buf + 200), RXSUBCOM_OK);
        assert_eq!(l.guest_read::<u16>(buf + 100), Some(RXSUBCOM_ISREG));
        assert_eq!(l.guest_read_bytes(buf + 200, 8).unwrap(), b"USERAREA");
        let h = l.rexx_api().subcoms.get(b"TESTENV".as_slice()).cloned().unwrap();
        assert_eq!(l.rexx_handler_address(&h), Some(0x1234));
        assert_eq!(l.rexx_deregister(Kind::Subcom, buf), RXSUBCOM_OK);
        assert_eq!(l.rexx_query(Kind::Subcom, buf, buf + 100, 0), RXSUBCOM_NOTREG);
        assert_eq!(l.guest_read::<u16>(buf + 100), Some(0));
        l.guest_write_bytes(buf + 300, b"NOSUCH.DLL\0");
        l.guest_write_bytes(buf + 400, b"Entry\0");
        assert_eq!(l.rexx_register_dll(Kind::Exit, buf, buf + 300, buf + 400, 0), RXEXIT_OK);
        let h = l.rexx_api().exits.get(b"TESTENV".as_slice()).cloned().unwrap();
        assert_eq!(l.rexx_handler_address(&h), None, "a DLL not loaded has no handler");
    }

    #[test]
    fn queues_hold_lines() {
        let (l, buf) = with_memory(4096);
        l.guest_write_bytes(buf, b"myq\0");
        assert_eq!(l.rexx_create_queue(buf + 100, 64, buf, buf + 200), RXQUEUE_OK);
        assert_eq!(l.guest_read_bytes(buf + 100, 4).unwrap(), b"MYQ\0");
        assert_eq!(l.rexx_create_queue(buf + 100, 64, buf, buf + 200), RXQUEUE_DUP);
        assert_eq!(l.guest_read::<u32>(buf + 200), Some(1), "another name was chosen");
        assert_eq!(l.rexx_create_queue(buf + 100, 2, 0, 0), RXQUEUE_SIZE, "the buffer is too small");
        // RXSTRINGs: a line to add, an empty one to pull into.
        l.guest_write_bytes(buf + 700, b"abc");
        l.guest_write::<u32>(buf + 600, 3);
        l.guest_write::<u32>(buf + 604, buf + 700);
        assert_eq!(l.rexx_add_queue(buf, buf + 600, RXQUEUE_FIFO), RXQUEUE_OK);
        assert_eq!(l.rexx_add_queue(buf, buf + 600, 7), RXQUEUE_PRIORITY);
        assert_eq!(l.rexx_query_queue(buf, buf + 300), RXQUEUE_OK);
        assert_eq!(l.guest_read::<u32>(buf + 300), Some(1));
        assert_eq!(l.rexx_pull_queue(buf, buf + 800, 0, RXQUEUE_NOWAIT), RXQUEUE_OK);
        assert_eq!(l.rx_string(buf + 800).unwrap(), b"abc", "new memory for an empty RXSTRING");
        assert_eq!(l.rexx_pull_queue(buf, buf + 800, 0, RXQUEUE_NOWAIT), RXQUEUE_EMPTY);
        assert_eq!(l.rexx_pull_queue(buf, buf + 800, 0, 5), RXQUEUE_BADWAITFLAG);
        assert_eq!(l.rexx_delete_queue(buf), RXQUEUE_OK);
        assert_eq!(l.rexx_query_queue(buf, buf + 300), RXQUEUE_NOTREG);
        l.guest_write_bytes(buf + 900, b"SESSION\0");
        assert_eq!(l.rexx_delete_queue(buf + 900), RXQUEUE_BADQNAME);
        assert_eq!(l.rexx_query_queue(buf + 900, buf + 300), RXQUEUE_OK, "the session queue is always there");
    }

    #[test]
    fn the_pool_and_signals_need_a_program() {
        let (l, _) = with_memory(64);
        assert_eq!(l.rexx_variable_pool(1, 0), RXSHV_NOAVL);
        assert_eq!(l.rexx_signal(0xFFFF, 1, RexxSignal::Halt), RXARI_NOT_FOUND);
        assert_eq!(l.rexx_macro_space("RexxQueryMacro"), RXMACRO_NOT_FOUND);
    }

    #[test]
    fn queue_names_are_checked() {
        assert_eq!(queue_name(b"myQueue"), Some(b"MYQUEUE".to_vec()));
        assert_eq!(queue_name(b"a b"), None);
        assert_eq!(queue_name(b""), None);
    }
}
