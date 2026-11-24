// SPDX-License-Identifier: GPL-3.0-or-later
//
// What a child process started by `DosExecPgm` gets from its parent: the
// command line, the environment and the inheritable pipe handles. A child
// is a separate Warpine process, so these go through the host `exec`:
//
// - `WARPINE_OS2ARG0` / `WARPINE_OS2ARGS` — the two strings of DosExecPgm's
//   argument block (program name, arguments), escaped (`escape`);
// - `WARPINE_OS2ENV` — the environment block (DosExecPgm's, or the
//   parent's own for a NULL `pEnv`), its strings separated by escaped NULs;
// - `WARPINE_HANDLES` — the pipe handles the child inherits:
//   `handle,fd,a,fhstate` for an anonymous pipe, and
//   `handle,fd,c,fhstate,openmode,pipemode,cbout,cbin,timeout,instances,namehex`
//   for the client end of a named pipe. The host descriptors stay
//   close-on-exec in the parent; only the child being started gets them
//   (`pre_exec` clears the flag in the forked child just before `exec`), so
//   no other process — and no other child started at the same time —
//   receives them. Standard handles 0–2 also go to the child's host
//   standard streams.
//
// - `WARPINE_PID` / `WARPINE_PPID` — the child's process id, which the
//   parent chose (`admit_child`), and the parent's own: the child reports
//   them in `pib_ulpid` / `pib_ulppid`, and the parent uses the same id in
//   `DosExecPgm`'s result, `DosWaitChild`, `DosKillProcess` and
//   `DosSendSignalException`. A process without them (the first of a
//   session) is `PROCESS_ID` with parent `NO_PARENT_PROCESS_ID`. Ids come
//   from a counter in the session's private directory
//   (`session_threads.rs`), so they are unique within the session.
// - `WARPINE_CLOCK_OFFSET` — the parent's clock offset (`clock.rs`), so a
//   `DosSetDateTime` before the start reaches the child (ADR-0039).
//
// The server end of a named pipe is not passed on (unverified whether OS/2
// lets a child serve its parent's instance; Warpine keeps it in the
// process that created it).

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::Arc;

use log::{debug, warn};

use super::managers::{PipeHandle, PipeObject};
use super::mutex_ext::MutexExt;
use super::npipe::{self, PipeSpec};
use super::constants::{FIRST_CHILD_PROCESS_ID, MAX_PROCESS_ID, NO_PARENT_PROCESS_ID, PROCESS_ID};
use super::session_threads::Reservation;

pub const HANDLES_ENV: &str = "WARPINE_HANDLES";
pub const ARG0_ENV: &str = "WARPINE_OS2ARG0";
pub const ARGS_ENV: &str = "WARPINE_OS2ARGS";
pub const ENV_ENV: &str = "WARPINE_OS2ENV";
/// The child's process id (decimal).
pub const PID_ENV: &str = "WARPINE_PID";
/// The parent's process id (decimal).
pub const PPID_ENV: &str = "WARPINE_PPID";
/// The OS/2 current drive and directories a child starts with
/// (`cwd_strings`, escaped).
pub const CWD_ENV: &str = "WARPINE_OS2CWD";
/// Most bytes of `CWD_ENV` read: 26 drives with a full path each.
const MAX_CWD_BLOCK: usize = 26 * 262;
/// Largest environment block passed on or built (an OS/2 environment
/// segment is at most 64 KiB).
pub const MAX_ENV_BLOCK: usize = 64 * 1024;
/// Largest argument string passed on.
pub const MAX_ARGS: usize = 32 * 1024;
/// Most handles passed to a child.
const MAX_INHERITED: usize = 256;
/// Escape byte of `escape`.
const ESC: u8 = 0x01;

/// The command line and environment the process was started with.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupInfo {
    /// The program-name string of the command line (else the program's
    /// OS/2 path).
    pub arg0: Option<Vec<u8>>,
    /// The argument string.
    pub args: Vec<u8>,
    /// The environment's strings (else Warpine's default environment).
    pub env: Option<Vec<Vec<u8>>>,
    /// The process id the parent chose (else `PROCESS_ID`).
    pub pid: Option<u32>,
    /// The parent's process id (else `NO_PARENT_PROCESS_ID`).
    pub ppid: Option<u32>,
    /// The current drive and directories the parent had (`cwd_strings`),
    /// else the root of drive C.
    pub cwd: Option<Vec<u8>>,
}

impl StartupInfo {
    /// From the variables a parent Warpine process set.
    pub fn from_env() -> Self {
        let get = |name: &str| std::env::var_os(name).map(|v| unescape(&v));
        StartupInfo {
            arg0: get(ARG0_ENV).filter(|a| !a.is_empty()),
            args: get(ARGS_ENV).map(|mut a| { a.truncate(MAX_ARGS); a }).unwrap_or_default(),
            env: get(ENV_ENV).map(|e| split_env(&e)),
            pid: parse_process_id(std::env::var_os(PID_ENV).as_deref()),
            ppid: parse_process_id(std::env::var_os(PPID_ENV).as_deref()),
            cwd: get(CWD_ENV).map(|mut c| { c.truncate(MAX_CWD_BLOCK); c }),
        }
    }

    /// This process's id.
    pub fn process_id(&self) -> u32 {
        self.pid.unwrap_or(PROCESS_ID)
    }

    /// The parent's id: only with an id of its own from that parent.
    pub fn parent_process_id(&self) -> u32 {
        self.pid.and(self.ppid).unwrap_or(NO_PARENT_PROCESS_ID)
    }

    /// Whether a parent passed the command line (else the host's own
    /// arguments are used).
    pub fn from_parent() -> bool {
        std::env::var_os(ARGS_ENV).is_some()
    }
}

/// A process id passed in `WARPINE_PID` / `WARPINE_PPID`: decimal numbers
/// only, 1 … `MAX_PROCESS_ID`; anything else is ignored.
pub fn parse_process_id(value: Option<&OsStr>) -> Option<u32> {
    let s = value?.to_str()?;
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    s.parse::<u32>().ok().filter(|&p| (1..=MAX_PROCESS_ID).contains(&p))
}

/// The id after `id` in a session's numbering (`FIRST_CHILD_PROCESS_ID` …
/// `MAX_PROCESS_ID`, then from the start again).
pub fn next_process_id(id: u32) -> u32 {
    if (FIRST_CHILD_PROCESS_ID..MAX_PROCESS_ID).contains(&id) { id + 1 } else { FIRST_CHILD_PROCESS_ID }
}

// ── Process nesting guard ───────────────────────────────────────────────────
//
// Every child is a whole Warpine process (a VM and its guest memory, some
// hundreds of MB), so a program that starts itself — on purpose or because
// it misreads its command line — could exhaust the host. Each child gets
// its depth in `WARPINE_DEPTH` (the first process is depth 0, its children
// 1, …); a process at `MAX_PROCESS_DEPTH` starts no more children, and a
// process started deeper (or with a malformed value) refuses to run guest
// code at all. A process also has at most `MAX_LIVE_CHILDREN` children it
// keeps track of at once (`DosExecPgm` asynchronous children not yet waited
// for, and `WinStartApp` applications). The starting call fails:
// `DosExecPgm` with `ERROR_NO_PROC_SLOTS`, `WinStartApp` with
// `PMERR_CANNOT_START`, CMD.EXE with a message.

/// Environment variable carrying a child process's nesting depth.
pub const DEPTH_ENV: &str = "WARPINE_DEPTH";
/// Deepest process nesting: a process at this depth starts no children.
pub const MAX_PROCESS_DEPTH: u32 = 8;
/// Most children a process keeps track of at once.
pub const MAX_LIVE_CHILDREN: usize = 16;
/// `ERROR_NO_PROC_SLOTS` (`<bseerr.h>`): `DosExecPgm`'s error at a limit.
pub const ERROR_NO_PROC_SLOTS: u32 = 89;
/// Exit code of a Warpine process started past `MAX_PROCESS_DEPTH`.
pub const EXIT_NESTING_LIMIT: i32 = 89;

/// The nesting depth in `value` (the `WARPINE_DEPTH` of this process):
/// none is depth 0; a malformed value counts as past the limit.
pub fn parse_depth(value: Option<&OsStr>) -> u32 {
    match value {
        None => 0,
        Some(v) => v.to_str().and_then(|s| s.parse::<u32>().ok())
            .map_or(MAX_PROCESS_DEPTH + 1, |d| d.min(MAX_PROCESS_DEPTH + 1)),
    }
}

/// This process's nesting depth.
pub fn own_depth() -> u32 {
    parse_depth(std::env::var_os(DEPTH_ENV).as_deref())
}

/// The depth a child of a process at `depth` gets, or `None` when it may
/// not start one.
pub fn child_depth(depth: u32) -> Option<u32> {
    (depth < MAX_PROCESS_DEPTH).then_some(depth + 1)
}

/// Whether a process at `depth` may run at all.
pub fn depth_allowed(depth: u32) -> bool {
    depth <= MAX_PROCESS_DEPTH
}

/// Why a child cannot be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildLimit {
    /// This process is at `MAX_PROCESS_DEPTH`.
    Depth,
    /// It already has `MAX_LIVE_CHILDREN` children.
    Live,
    /// The session runs as many threads as `THREADS=` allows
    /// (`session_threads.rs`): no slot for the child's thread 1.
    Threads,
}

/// Check both limits for a process at `depth` with `live` children and set
/// the child's depth on `cmd`.
pub fn set_child_depth(cmd: &mut std::process::Command, depth: u32, live: usize) -> Result<(), ChildLimit> {
    if live >= MAX_LIVE_CHILDREN {
        return Err(ChildLimit::Live);
    }
    let d = child_depth(depth).ok_or(ChildLimit::Depth)?;
    cmd.env(DEPTH_ENV, d.to_string());
    Ok(())
}

/// Children the process of `shared` keeps track of now (`DosExecPgm`
/// asynchronous children, `WinStartApp` applications).
pub(crate) fn live_children(shared: &super::SharedState) -> usize {
    let execs = shared.process_mgr.lock_or_recover().children.len();
    execs + shared.window_mgr.lock_or_recover().apps.len()
}

/// A child allowed to start: its slot in the session's thread accounting
/// and the process id it gets.
#[derive(Debug)]
#[must_use = "an admission is settled with child_started"]
pub struct Admission {
    slot: Reservation,
    /// The pipe the child reports its exit code on (`exit_report.rs`).
    exit: Option<super::exit_report::ExitPipe>,
    /// The child's OS/2 process id (`WARPINE_PID`).
    pub pid: u32,
}

/// Allow a new child of the process of `shared` (see the nesting guard
/// above), give `cmd` its depth, reserve the slot of its thread 1 in the
/// session's thread accounting (`session_threads.rs`) and choose its process
/// id (`WARPINE_PID`, with this process's own in `WARPINE_PPID`). The
/// admission is settled with [`child_started`] once the spawn succeeded or
/// failed.
pub(crate) fn admit_child(shared: &super::SharedState, cmd: &mut std::process::Command)
    -> Result<Admission, ChildLimit>
{
    let r = set_child_depth(cmd, own_depth(), live_children(shared));
    if let Err(limit) = r {
        warn!("not starting a child process: {limit:?} limit (depth {} of {MAX_PROCESS_DEPTH}, \
               at most {MAX_LIVE_CHILDREN} children)", own_depth());
        return Err(limit);
    }
    let pid = shared.allocate_process_id();
    let slot = shared.thread_slots.lock_or_recover().reserve_for(cmd, pid).map_err(|_| {
        warn!("not starting a child process: the session is at its THREADS= limit");
        ChildLimit::Threads
    })?;
    cmd.env(PID_ENV, pid.to_string());
    cmd.env(PPID_ENV, shared.process_id().to_string());
    // The child's clock starts where this process's is (`clock.rs`); it also
    // overrides a `WARPINE_CLOCK` start date the child would otherwise reread.
    cmd.env(super::clock::CLOCK_OFFSET_ENV, shared.clock.offset_ms().to_string());
    // The session's shared arena (`shared_arena.rs`).
    match shared.arena.get() {
        Some(arena) => arena.pass_to(cmd),
        None => { cmd.env_remove(super::shared_arena::ARENA_FD_ENV); }
    }
    match shared.shared_sems.get() {
        Some(sems) => super::shared_sem::pass_to(sems, cmd),
        None => { cmd.env_remove(super::shared_sem::SEM_FD_ENV); }
    }
    // Its whole exit code comes back on a pipe (`exit_report.rs`).
    let exit = super::exit_report::ExitPipe::new().ok();
    match &exit {
        Some(p) => p.pass_to(cmd),
        None => { cmd.env_remove(super::exit_report::EXIT_FD_ENV); }
    }
    Ok(Admission { slot, pid, exit })
}

/// The child of `adm` started as host process `pid` (`None`: it did not
/// start): its reserved slot becomes its own, or is given back.
pub(crate) fn child_started(shared: &super::SharedState, adm: Admission, pid: Option<u32>) {
    shared.thread_slots.lock_or_recover().settle(adm.slot, pid);
    // Keep the read end of its exit report until it is reaped.
    if let (Some(host), Some(pipe)) = (pid, adm.exit) {
        shared.exit_reports.lock_or_recover().insert(host, pipe.into_read());
    }
}

impl super::SharedState {
    /// The exit code of child host process `host` that ended with
    /// `status`: its whole report when it sent one (`exit_report.rs`), else
    /// the host status; `None` when killed by a host signal. Every reaping
    /// path calls it, so the report's pipe is closed, and the session's
    /// shared blocks and semaphores are swept at once: a child killed
    /// before it could let them go leaves nothing behind (ADR-0049 and
    /// ADR-0050 addenda).
    pub(crate) fn child_exit_code(&self, host: u32, status: std::process::ExitStatus) -> Option<u32> {
        let read = self.exit_reports.lock_or_recover().remove(&host);
        let code = super::exit_report::full_code(read, status);
        self.sweep_after_child();
        code
    }

    /// Drop the holders and openers of the session's shared blocks and
    /// semaphores that are gone (a child just reaped among them).
    pub(crate) fn sweep_after_child(&self) {
        if self.arena.get().is_some() {
            let _ = self.with_arena_table(|_, _, _| ());
        }
        if let Some(sems) = self.shared_sems.get() {
            sems.with_lock(|l| l.sweep());
        }
    }

    /// This process's OS/2 id (`pib_ulpid`).
    pub fn process_id(&self) -> u32 {
        self.startup.lock_or_recover().process_id()
    }

    /// The parent's OS/2 id (`pib_ulppid`).
    pub fn parent_process_id(&self) -> u32 {
        self.startup.lock_or_recover().parent_process_id()
    }

    /// A new id for a child: the session's next one, or — when the session
    /// directory cannot be used — the next one this process has not given
    /// out (unique among its own children only).
    pub(crate) fn allocate_process_id(&self) -> u32 {
        let session = self.thread_slots.lock_or_recover().next_process_id();
        session.unwrap_or_else(|| {
            let avoid = [self.process_id(), self.parent_process_id()];
            self.process_mgr.lock_or_recover().alloc_pid(&avoid)
        })
    }
}

impl super::Loader {
    /// [`admit_child`] for this process.
    pub(crate) fn admit_child(&self, cmd: &mut std::process::Command) -> Result<Admission, ChildLimit> {
        admit_child(&self.shared, cmd)
    }

    /// [`child_started`] for this process.
    pub(crate) fn child_started(&self, adm: Admission, pid: Option<u32>) {
        child_started(&self.shared, adm, pid)
    }

    /// Put this process's id and its parent's into the PIB (`pib_ulpid`,
    /// `pib_ulppid`).
    pub(crate) fn write_pib_process_ids(&self) {
        use super::constants::{PIB_BASE, PIB_PID_OFFSET, PIB_PPID_OFFSET};
        self.guest_write::<u32>(PIB_BASE + PIB_PID_OFFSET, self.shared.process_id())
            .expect("PIB pib_ulpid out of guest memory");
        self.guest_write::<u32>(PIB_BASE + PIB_PPID_OFFSET, self.shared.parent_process_id())
            .expect("PIB pib_ulppid out of guest memory");
    }
}

/// Remove the variables a parent passed to this process from `cmd`, so a
/// child started without `DosExecPgm`'s set-up (the built-in CMD.EXE) does
/// not take this process's command line, environment or handles.
pub fn clear_parent_vars(cmd: &mut std::process::Command) {
    for var in [HANDLES_ENV, ARG0_ENV, ARGS_ENV, ENV_ENV, CWD_ENV] {
        cmd.env_remove(var);
    }
}

/// Escape `bytes` for an environment variable: NUL → `01 30`, `01` → `01 31`.
pub fn escape(bytes: &[u8]) -> OsString {
    let mut out = Vec::with_capacity(bytes.len() + 8);
    for &b in bytes {
        match b {
            0 => out.extend_from_slice(&[ESC, b'0']),
            ESC => out.extend_from_slice(&[ESC, b'1']),
            _ => out.push(b),
        }
    }
    OsString::from_vec(out)
}

/// Undo `escape` (a stray escape byte is kept as it is).
pub fn unescape(s: &OsStr) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1)) {
            (ESC, Some(b'0')) => { out.push(0); i += 2; }
            (ESC, Some(b'1')) => { out.push(ESC); i += 2; }
            (b, _) => { out.push(b); i += 1; }
        }
    }
    out
}

// ── Current drive and directories ─────────────────────────────────────────
//
// OS/2 starts a child with its parent's current drive and the current
// directory of every drive. Each Warpine process has its own DriveManager,
// so the parent passes them in `CWD_ENV`: `X:\dir` strings separated by
// NULs, the current drive first. The child sets each through its own VFS
// (which checks that the directory exists inside its drive) and drops what
// fails. The host's working directory has nothing to do with it.

/// The `CWD_ENV` value for a child of a process with `dm`. `start`, an
/// OS/2 directory, replaces the current drive and its directory (the
/// start directory of `WinStartApp`); one that does not resolve is ignored.
pub fn cwd_strings(dm: &super::vfs::DriveManager, start: Option<&str>) -> Vec<u8> {
    let entry = |drive: u8, rel: &str| format!("{}:\\{}", (b'A' + drive) as char, rel.replace('/', "\\"));
    let start = start.and_then(|s| {
        let (drive, rel) = dm.resolve_path(s).ok()?;
        Some((drive, super::vfs::normalize_rel_path(&rel)?))
    });
    let first = start.as_ref().map_or(dm.current_disk(), |(d, _)| *d);
    let mut entries = vec![match &start {
        Some((d, rel)) => entry(*d, rel),
        None => entry(first, dm.current_dir(first)),
    }];
    let mounted = dm.logical_drive_map();
    for drive in (0..26u8).filter(|&d| d != first && mounted & (1 << d) != 0) {
        entries.push(entry(drive, dm.current_dir(drive)));
    }
    entries.join("\0").into_bytes()
}

/// Set `dm`'s current directories and drive from a `cwd_strings` value.
/// A directory that is gone, or a drive this process lacks, is skipped.
pub fn apply_cwd(dm: &mut super::vfs::DriveManager, block: &[u8]) {
    let block = &block[..block.len().min(MAX_CWD_BLOCK)];
    let mut first_drive = None;
    for entry in block.split(|&b| b == 0).take(26) {
        let Ok(entry) = std::str::from_utf8(entry) else { continue };
        let Some(letter) = entry.bytes().next().filter(u8::is_ascii_alphabetic) else { continue };
        if !entry[1..].starts_with(":\\") { continue; }
        match dm.set_current_dir(entry) {
            Ok(()) => {}
            Err(e) => debug!("inherited current directory {entry} not set: {e:?}"),
        }
        first_drive.get_or_insert(letter.to_ascii_uppercase() - b'A' + 1);
    }
    if let Some(disk) = first_drive && dm.set_current_disk(disk).is_err() {
        debug!("inherited current drive {disk} is not mounted");
    }
}

/// The strings of an environment block (`NAME=value\0…\0`), up to the
/// empty string or the end.
pub fn split_env(block: &[u8]) -> Vec<Vec<u8>> {
    let block = &block[..block.len().min(MAX_ENV_BLOCK)];
    block.split(|&b| b == 0).take_while(|s| !s.is_empty()).map(<[u8]>::to_vec).collect()
}

/// An environment block for `strings` (each NUL-terminated, then a NUL).
pub fn join_env(strings: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in strings {
        if out.len() + s.len() + 2 > MAX_ENV_BLOCK { break; }
        out.extend_from_slice(s);
        out.push(0);
    }
    out.push(0);
    out
}

/// The process block the guest finds at `pib_pchenv`: the environment,
/// then the program's full name, then the command line (`arg0\0args\0\0`)
/// at the returned offset (`pib_pchcmd`). OS/2 programs find their full
/// name as the string before the command line (4OS2 does).
pub fn process_block(env: &[Vec<u8>], program: &[u8], arg0: &[u8], args: &[u8]) -> (Vec<u8>, usize) {
    let mut block = join_env(env);
    block.extend_from_slice(program);
    block.push(0);
    let cmd = block.len();
    block.extend_from_slice(arg0);
    block.push(0);
    block.extend_from_slice(&args[..args.len().min(MAX_ARGS)]);
    block.push(0);
    block.push(0);
    (block, cmd)
}

/// One handle passed to a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inherited {
    pub handle: u32,
    pub fd: RawFd,
    pub fh_state: u32,
    /// The named pipe of a client end; `None` for an anonymous pipe.
    pub client: Option<(PipeSpec, u32)>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) { return None; }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// `WARPINE_HANDLES` for `list`.
pub fn encode_handles(list: &[Inherited]) -> String {
    list.iter().map(|i| match &i.client {
        None => format!("{},{},a,{}", i.handle, i.fd, i.fh_state),
        Some((s, cur)) => format!("{},{},c,{},{},{},{},{},{},{},{}", i.handle, i.fd, i.fh_state,
            s.open_mode, s.pipe_mode, s.cb_out, s.cb_in, s.timeout, cur, hex(s.name.as_bytes())),
    }).collect::<Vec<_>>().join(";")
}

/// Parse `WARPINE_HANDLES`; malformed entries are left out.
pub fn decode_handles(s: &str) -> Vec<Inherited> {
    s.split(';').filter(|e| !e.is_empty()).take(MAX_INHERITED).filter_map(|e| {
        let f: Vec<&str> = e.split(',').collect();
        let n = |i: usize| f.get(i).and_then(|v| v.parse::<u32>().ok());
        let handle = n(0)?;
        let fd: RawFd = f.get(1)?.parse().ok().filter(|&fd: &RawFd| fd >= 0)?;
        let fh_state = n(3)?;
        let client = match *f.get(2)? {
            "a" if f.len() == 4 => None,
            "c" if f.len() == 11 => {
                let name = String::from_utf8(unhex(f[10])?).ok().filter(|n| npipe::is_pipe_name(n))?;
                Some((PipeSpec { name, open_mode: n(4)?, pipe_mode: n(5)?, cb_out: n(6)?,
                                 cb_in: n(7)?, timeout: n(8)? }, n(9)?))
            }
            _ => return None,
        };
        Some(Inherited { handle, fd, fh_state, client })
    }).collect()
}

/// Whether `fd` is an open pipe or socket (what a parent passes).
fn is_pipe_fd(fd: RawFd) -> bool {
    // SAFETY: `st` is a plain struct fstat fills in; an invalid fd fails.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return false;
    }
    let kind = st.st_mode & libc::S_IFMT;
    kind == libc::S_IFIFO || kind == libc::S_IFSOCK
}

/// The pipe handles a child process inherits, with their descriptors.
pub struct ChildHandles {
    /// Duplicates owned until the child has started.
    owned: Vec<OwnedFd>,
    list: Vec<Inherited>,
}

impl ChildHandles {
    /// The value of `WARPINE_HANDLES`.
    pub fn spec(&self) -> String {
        encode_handles(&self.list)
    }

    /// Descriptors the child keeps across `exec` (standard ones excluded:
    /// the command's standard streams carry those).
    pub fn passed_fds(&self) -> Vec<RawFd> {
        self.owned.iter().map(|f| f.as_raw_fd()).collect()
    }

    /// Set up `cmd` to pass the handles: the variable, and a `pre_exec`
    /// hook that clears close-on-exec on the passed descriptors in the
    /// forked child only.
    pub fn apply(&self, cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        if self.list.is_empty() {
            cmd.env_remove(HANDLES_ENV);
            return;
        }
        cmd.env(HANDLES_ENV, self.spec());
        let fds = self.passed_fds();
        if fds.is_empty() {
            return;
        }
        // SAFETY: the hook runs in the forked child before `exec`; it only
        // calls fcntl, which is async-signal-safe, on descriptors the
        // parent owns for the whole spawn (`self.owned`).
        unsafe {
            cmd.pre_exec(move || {
                for &fd in &fds {
                    if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
}

impl super::Loader {
    /// Collect the inheritable pipe handles for a child (`DosExecPgm`). An
    /// anonymous pipe at a standard handle goes as the child's host
    /// standard stream (`fd` 0–2); everything else as a duplicate.
    pub(crate) fn child_handles(&self) -> ChildHandles {
        let hm = self.shared.handle_mgr.lock_or_recover();
        let mut out = ChildHandles { owned: Vec::new(), list: Vec::new() };
        let mut entries: Vec<(u32, PipeHandle)> = hm.iter()
            .filter(|(_, e)| e.inheritable())
            .map(|(h, e)| (h, e.clone())).collect();
        drop(hm);
        entries.sort_by_key(|(h, _)| *h);
        for (h, e) in entries.into_iter().take(MAX_INHERITED) {
            let (dup, client): (std::io::Result<OwnedFd>, _) = match &e.obj {
                PipeObject::Host(_) if h <= 2 => {
                    out.list.push(Inherited { handle: h, fd: h as RawFd, fh_state: e.fh_state, client: None });
                    continue;
                }
                PipeObject::Host(f) => (f.try_clone().map(OwnedFd::from), None),
                PipeObject::Client(c) => (c.conn.try_clone_socket().map(OwnedFd::from),
                                          Some((c.spec.clone(), c.cur_inst))),
                // The server end stays in its process.
                PipeObject::Server(_) => continue,
            };
            match dup {
                Ok(fd) => {
                    out.list.push(Inherited { handle: h, fd: fd.as_raw_fd(), fh_state: e.fh_state, client });
                    out.owned.push(fd);
                }
                Err(err) => warn!("handle {h} not passed to the child: {err}"),
            }
        }
        out
    }

    /// Take over the pipe handles a parent passed (`WARPINE_HANDLES`).
    pub(crate) fn adopt_inherited_handles(&self) {
        let Some(spec) = std::env::var_os(HANDLES_ENV) else { return };
        let list = decode_handles(&spec.to_string_lossy());
        self.adopt_handles(&list);
    }

    pub(crate) fn adopt_handles(&self, list: &[Inherited]) {
        let mut taken = std::collections::HashSet::new();
        for i in list {
            if !is_pipe_fd(i.fd) {
                warn!("inherited handle {}: descriptor {} is not a pipe", i.handle, i.fd);
                continue;
            }
            // A standard stream stays the host's; the handle gets its own
            // descriptor. Others are taken over once.
            let fd = if i.fd <= 2 || !taken.insert(i.fd) {
                // SAFETY: F_DUPFD_CLOEXEC on a valid descriptor.
                let d = unsafe { libc::fcntl(i.fd, libc::F_DUPFD_CLOEXEC, 3) };
                if d < 0 { continue; }
                // SAFETY: `d` is a new descriptor nothing else owns.
                unsafe { OwnedFd::from_raw_fd(d) }
            } else {
                // SAFETY: the parent passed this descriptor for this handle
                // only; nothing else in this process owns it.
                let fd = unsafe { OwnedFd::from_raw_fd(i.fd) };
                // SAFETY: fcntl on the descriptor just taken over.
                unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
                fd
            };
            let obj = match &i.client {
                None => PipeObject::Host(Arc::new(std::fs::File::from(fd))),
                Some((spec, cur)) => {
                    let Ok(conn) = npipe::Conn::new(std::os::unix::net::UnixStream::from(fd)) else { continue };
                    PipeObject::Client(Arc::new(npipe::client_end(spec.clone(), *cur, conn)))
                }
            };
            debug!("inherited pipe handle {} (fd {})", i.handle, i.fd);
            self.put_pipe_handle_at(i.handle, PipeHandle { obj, fh_state: i.fh_state });
        }
    }

    /// The process block for `pib_pchenv` / `pib_pchcmd` (see
    /// `process_block`): the parent's environment or `default_env`, the
    /// program's OS/2 name, and the command line the parent gave (else
    /// the program name and the arguments from the host command line).
    pub(crate) fn startup_block(&self) -> (Vec<u8>, usize) {
        let os2_exe = self.os2_exe_path();
        let st = self.shared.startup.lock_or_recover().clone();
        let env = st.env.unwrap_or_else(|| self.shared.initial_environment());
        let arg0 = st.arg0.unwrap_or_else(|| os2_exe.as_bytes().to_vec());
        process_block(&env, os2_exe.as_bytes(), &arg0, &st.args)
    }

    /// Read an environment block from guest memory (bounded).
    pub(crate) fn read_env_block(&self, ptr: u32) -> Vec<Vec<u8>> {
        let mut block = Vec::new();
        let mut prev = 1u8;
        for off in 0..MAX_ENV_BLOCK as u32 {
            let Some(b) = ptr.checked_add(off).and_then(|a| self.guest_read::<u8>(a)) else { break };
            if b == 0 && (prev == 0 || off == 0) { break; }
            block.push(b);
            prev = b;
        }
        split_env(&block)
    }

    /// The two strings of a DosExecPgm argument block (bounded), as bytes.
    pub(crate) fn read_arg_block(&self, ptr: u32) -> (Vec<u8>, Vec<u8>) {
        let read = |start: u32| -> (Vec<u8>, u32) {
            let mut s = Vec::new();
            let mut off = 0u32;
            while (off as usize) < MAX_ARGS {
                match start.checked_add(off).and_then(|a| self.guest_read::<u8>(a)) {
                    Some(0) | None => break,
                    Some(b) => s.push(b),
                }
                off += 1;
            }
            (s, off)
        };
        if ptr == 0 { return (Vec::new(), Vec::new()); }
        let (arg0, len) = read(ptr);
        let (args, _) = ptr.checked_add(len + 1).map(read).unwrap_or_default();
        (arg0, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use super::super::constants::OPEN_FLAGS_NOINHERIT;
    use std::io::{Read, Write};

    #[test]
    fn depth_parses_and_bounds() {
        assert_eq!(parse_depth(None), 0);
        assert_eq!(parse_depth(Some(OsStr::new("3"))), 3);
        assert_eq!(parse_depth(Some(OsStr::new("junk"))), MAX_PROCESS_DEPTH + 1);
        assert_eq!(parse_depth(Some(OsStr::new("-1"))), MAX_PROCESS_DEPTH + 1);
        assert_eq!(parse_depth(Some(OsStr::new("99999999999"))), MAX_PROCESS_DEPTH + 1);
        assert_eq!(parse_depth(Some(OsStr::new("1000"))), MAX_PROCESS_DEPTH + 1);
        assert!(depth_allowed(MAX_PROCESS_DEPTH));
        assert!(!depth_allowed(MAX_PROCESS_DEPTH + 1));
        assert_eq!(child_depth(0), Some(1));
        assert_eq!(child_depth(MAX_PROCESS_DEPTH - 1), Some(MAX_PROCESS_DEPTH));
        assert_eq!(child_depth(MAX_PROCESS_DEPTH), None);
    }

    #[test]
    fn children_get_their_depth_within_the_limits() {
        let depth_of = |cmd: &std::process::Command| cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(DEPTH_ENV)).and_then(|(_, v)| v).map(|v| v.to_owned());
        let mut cmd = std::process::Command::new("true");
        assert_eq!(set_child_depth(&mut cmd, 2, 0), Ok(()));
        assert_eq!(depth_of(&cmd).as_deref(), Some(OsStr::new("3")));
        let mut cmd = std::process::Command::new("true");
        assert_eq!(set_child_depth(&mut cmd, MAX_PROCESS_DEPTH, 0), Err(ChildLimit::Depth));
        assert_eq!(depth_of(&cmd), None);
        assert_eq!(set_child_depth(&mut cmd, 0, MAX_LIVE_CHILDREN), Err(ChildLimit::Live));
        assert_eq!(set_child_depth(&mut cmd, 0, MAX_LIVE_CHILDREN - 1), Ok(()));
        // Clearing what the parent passed keeps the depth.
        clear_parent_vars(&mut cmd);
        assert!(depth_of(&cmd).is_some());
    }

    #[test]
    fn a_loader_counts_its_children() {
        let l = Loader::new_mock();
        assert_eq!(live_children(&l.shared), 0);
        let mut cmd = std::process::Command::new("true");
        assert!(l.admit_child(&mut cmd).is_ok() || own_depth() >= MAX_PROCESS_DEPTH);
    }

    #[test]
    fn process_ids_are_parsed_strictly() {
        let p = |s: &str| parse_process_id(Some(OsStr::new(s)));
        assert_eq!(p("100"), Some(100));
        assert_eq!(p("65535"), Some(MAX_PROCESS_ID));
        for bad in ["", "0", "65536", "-1", "+5", " 5", "5 ", "1e3", "999999999999", "\u{663}"] {
            assert_eq!(p(bad), None, "{bad:?}");
        }
        assert_eq!(parse_process_id(None), None);
        assert_eq!(next_process_id(FIRST_CHILD_PROCESS_ID), FIRST_CHILD_PROCESS_ID + 1);
        assert_eq!(next_process_id(MAX_PROCESS_ID), FIRST_CHILD_PROCESS_ID);
        assert_eq!(next_process_id(PROCESS_ID), FIRST_CHILD_PROCESS_ID, "below the range");
        assert_eq!(next_process_id(u32::MAX), FIRST_CHILD_PROCESS_ID);
    }

    #[test]
    fn a_process_reports_the_ids_its_parent_gave() {
        let first = StartupInfo::default();
        assert_eq!((first.process_id(), first.parent_process_id()), (PROCESS_ID, NO_PARENT_PROCESS_ID));
        let child = StartupInfo { pid: Some(105), ppid: Some(101), ..Default::default() };
        assert_eq!((child.process_id(), child.parent_process_id()), (105, 101));
        // A parent id without an own id is not believed.
        let odd = StartupInfo { ppid: Some(101), ..Default::default() };
        assert_eq!(odd.parent_process_id(), NO_PARENT_PROCESS_ID);

        let l = Loader::new_mock();
        l.shared.startup.lock_or_recover().pid = Some(105);
        l.shared.startup.lock_or_recover().ppid = Some(101);
        l.write_pib_process_ids();
        use super::super::constants::{PIB_BASE, PIB_PID_OFFSET, PIB_PPID_OFFSET};
        assert_eq!(l.guest_read::<u32>(PIB_BASE + PIB_PID_OFFSET), Some(105));
        assert_eq!(l.guest_read::<u32>(PIB_BASE + PIB_PPID_OFFSET), Some(101));
    }

    #[test]
    fn an_admitted_child_gets_its_id_and_its_parents() {
        if own_depth() >= MAX_PROCESS_DEPTH { return; }
        let l = Loader::new_mock();
        l.shared.startup.lock_or_recover().pid = Some(PROCESS_ID + 1);
        let env_of = |cmd: &std::process::Command, name: &str| cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(name)).and_then(|(_, v)| v)
            .and_then(|v| v.to_str()).map(str::to_owned);
        let mut cmd = std::process::Command::new("true");
        let a = l.admit_child(&mut cmd).unwrap();
        assert_eq!(a.pid, FIRST_CHILD_PROCESS_ID);
        assert_eq!(env_of(&cmd, PID_ENV), Some(a.pid.to_string()));
        assert_eq!(env_of(&cmd, PPID_ENV), Some((PROCESS_ID + 1).to_string()));
        assert_eq!(env_of(&cmd, super::super::clock::CLOCK_OFFSET_ENV), Some("0".into()));
        l.shared.clock.set_now_ms(4_102_444_800_000); // 2100-01-01 UTC
        let mut cmd3 = std::process::Command::new("true");
        let c = l.admit_child(&mut cmd3).unwrap();
        let offset: i64 = env_of(&cmd3, super::super::clock::CLOCK_OFFSET_ENV).unwrap().parse().unwrap();
        assert!((offset - l.shared.clock.offset_ms()).abs() < 1000, "the child gets the offset");
        l.child_started(c, None);
        let mut cmd2 = std::process::Command::new("true");
        let b = l.admit_child(&mut cmd2).unwrap();
        assert_ne!(a.pid, b.pid);
        l.child_started(a, None);
        l.child_started(b, None);
    }

    #[test]
    fn fallback_ids_avoid_this_process_and_its_children() {
        let l = Loader::new_mock();
        // Without a session (unit-test loaders) ids come from this process.
        l.shared.startup.lock_or_recover().pid = Some(FIRST_CHILD_PROCESS_ID);
        l.shared.startup.lock_or_recover().ppid = Some(FIRST_CHILD_PROCESS_ID + 1);
        let child = std::process::Command::new("true").spawn().unwrap();
        l.shared.process_mgr.lock_or_recover().insert_child(FIRST_CHILD_PROCESS_ID + 2, child);
        assert_eq!(l.shared.allocate_process_id(), FIRST_CHILD_PROCESS_ID + 3);
        assert_eq!(l.shared.allocate_process_id(), FIRST_CHILD_PROCESS_ID + 4);
        let mut child = l.shared.process_mgr.lock_or_recover().take_child(FIRST_CHILD_PROCESS_ID + 2).unwrap();
        let _ = child.wait();
    }

    #[test]
    fn escape_round_trip() {
        let raw = b"A=1\0B=\x01x\0\0\xff".to_vec();
        let e = escape(&raw);
        assert!(!e.as_bytes().contains(&0));
        assert_eq!(unescape(&e), raw);
        assert_eq!(unescape(OsStr::new("a\x01")), b"a\x01", "stray escape");
    }

    #[test]
    fn env_blocks() {
        let env = split_env(b"A=1\0PATH=C:\\\0\0junk");
        assert_eq!(env, vec![b"A=1".to_vec(), b"PATH=C:\\".to_vec()]);
        assert_eq!(join_env(&env), b"A=1\0PATH=C:\\\0\0");
        assert_eq!(join_env(&[]), b"\0");
        let huge = vec![vec![b'x'; MAX_ENV_BLOCK]];
        assert_eq!(join_env(&huge), b"\0", "an oversized string is left out");
    }

    /// The program's full name sits right before the command line.
    #[test]
    fn process_block_layout() {
        let (block, cmd) = process_block(&[b"A=1".to_vec()], b"C:\\P.EXE", b"P", b"x y");
        assert_eq!(block, b"A=1\0\0C:\\P.EXE\0P\0x y\0\0");
        assert_eq!(&block[cmd..cmd + 2], b"P\0");
        // 4OS2's way: back from the command line to the NUL before the name.
        let mut i = cmd - 1;
        while block[i - 1] != 0 { i -= 1; }
        assert_eq!(&block[i..cmd - 1], b"C:\\P.EXE");
    }

    #[test]
    fn handle_specs_round_trip() {
        let list = vec![
            Inherited { handle: 0, fd: 0, fh_state: 0, client: None },
            Inherited { handle: 0x1003, fd: 7, fh_state: 2, client: Some((PipeSpec {
                name: "\\PIPE\\A,B".into(), open_mode: 2, pipe_mode: 0x401, cb_out: 1, cb_in: 2, timeout: 50,
            }, 1)) },
        ];
        assert_eq!(decode_handles(&encode_handles(&list)), list);
        assert!(decode_handles("x;1,-1,a,0;1,3,a;2,3,z,0;3,4,c,0,0,0,0,0,0,0,zz").is_empty());
        assert_eq!(decode_handles("").len(), 0);
    }

    #[test]
    fn parent_vars_are_cleared() {
        let mut cmd = std::process::Command::new("true");
        cmd.env(HANDLES_ENV, "x").env(ARGS_ENV, "y");
        clear_parent_vars(&mut cmd);
        let set: Vec<_> = cmd.get_envs().filter(|(_, v)| v.is_some()).collect();
        assert!(set.is_empty(), "{set:?}");
    }

    /// A child gets its parent's current drive and directories; one that
    /// is gone, a path out of the drive or junk is dropped, never applied.
    #[test]
    fn current_directories_pass_to_a_child() {
        use super::super::vfs::DriveManager;
        use super::super::vfs_hostdir::HostDirBackend;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("work/src")).unwrap();
        let drive = || Box::new(HostDirBackend::new(root.path().to_path_buf()).unwrap());
        let mut parent = DriveManager::new();
        parent.mount(2, drive());
        parent.mount(3, drive());
        parent.set_current_disk(4).unwrap();
        parent.set_current_dir("C:\\WORK\\SRC").unwrap();
        parent.set_current_dir("D:\\WORK").unwrap();
        let block = cwd_strings(&parent, None);
        assert_eq!(block, b"D:\\WORK\0C:\\WORK\\SRC");
        assert_eq!(unescape(&escape(&block)), block);

        let mut child = DriveManager::new();
        child.mount(2, drive());
        child.mount(3, drive());
        apply_cwd(&mut child, &block);
        assert_eq!(child.current_disk(), 3, "the parent's current drive");
        assert_eq!(child.current_dir(2), "WORK/SRC");
        assert_eq!(child.current_dir(3), "WORK");

        // WinStartApp's start directory replaces the current drive's.
        assert_eq!(cwd_strings(&parent, Some("C:\\WORK")), b"C:\\WORK\0D:\\WORK");
        assert_eq!(cwd_strings(&parent, Some("Q:\\NONE")), block, "an unknown drive is ignored");

        let mut other = DriveManager::new();
        other.mount(2, drive());
        apply_cwd(&mut other, b"C:\\GONE\0C:\\..\\..\0Z:\\X\0junk\0\xff\xfe");
        assert_eq!((other.current_disk(), other.current_dir(2)), (2, ""), "nothing applied");
        apply_cwd(&mut other, b"Z:\\X\0C:\\WORK");
        assert_eq!((other.current_disk(), other.current_dir(2)), (2, "WORK"), "a missing drive stays unchosen");
    }

    /// Arguments and environment from a parent (`from_env` reads them).
    #[test]
    fn startup_from_parent_strings() {
        let info = StartupInfo {
            arg0: Some(unescape(&escape(b"FIND"))),
            args: unescape(&escape(b"\"Warp\"")),
            env: Some(split_env(&unescape(&escape(b"A=1\0B=2\0")))),
            ..Default::default()
        };
        assert_eq!(info.env.as_ref().unwrap().len(), 2);
        assert_eq!(info.args, b"\"Warp\"");
    }

    /// Guest blocks are read with bounds.
    #[test]
    fn guest_blocks() {
        let l = Loader::new_mock();
        l.guest_write_bytes(0x3000, b"PROG\0a b\0\0").unwrap();
        assert_eq!(l.read_arg_block(0x3000), (b"PROG".to_vec(), b"a b".to_vec()));
        assert_eq!(l.read_arg_block(0), (vec![], vec![]));
        l.guest_write_bytes(0x3100, b"X=1\0Y=2\0\0").unwrap();
        assert_eq!(l.read_env_block(0x3100), vec![b"X=1".to_vec(), b"Y=2".to_vec()]);
        assert!(l.read_env_block(u32::MAX - 1).is_empty());
        assert!(l.read_arg_block(u32::MAX - 2).0.is_empty(), "outside guest memory");
    }

    /// Inheritable anonymous pipes are collected; non-inheritable ones and
    /// server ends are not; adopted handles work in the "child".
    #[test]
    fn collect_and_adopt() {
        let parent = Loader::new_mock();
        assert_eq!(parent.dos_create_pipe(0x4000, 0x4004, 0), 0);
        let hr = parent.guest_read::<u32>(0x4000).unwrap();
        let hw = parent.guest_read::<u32>(0x4004).unwrap();
        parent.shared.handle_mgr.lock().unwrap().entry_mut(hr).unwrap().fh_state |= OPEN_FLAGS_NOINHERIT;
        let ch = parent.child_handles();
        assert_eq!(ch.list.len(), 1, "only the write end");
        assert_eq!(ch.list[0].handle, hw);
        assert_eq!(ch.passed_fds().len(), 1);
        // Adopt a duplicate in another loader (as the child would).
        let child = Loader::new_mock();
        let dup = ch.owned[0].try_clone().unwrap();
        let fd = dup.as_raw_fd();
        std::mem::forget(dup); // ownership moves to the adopting loader
        child.adopt_handles(&[Inherited { handle: hw, fd, fh_state: 1, client: None }]);
        child.guest_write_bytes(0x5000, b"from child").unwrap();
        assert_eq!(child.dos_write(hw, 0x5000, 10, 0x5100), 0);
        drop(child);
        drop(ch);
        assert_eq!(parent.dos_close(hw), 0);
        let mut got = Vec::new();
        let f = parent.shared.handle_mgr.lock().unwrap().host_file(hr).unwrap();
        (&*f).read_to_end(&mut got).unwrap();
        assert_eq!(got, b"from child", "all write ends closed: end of data");
        // Not a pipe: refused.
        let file = tempfile::tempfile().unwrap();
        let l = Loader::new_mock();
        l.adopt_handles(&[Inherited { handle: 0x1000, fd: file.as_raw_fd(), fh_state: 0, client: None }]);
        assert!(!l.shared.handle_mgr.lock().unwrap().contains(0x1000));
    }

    /// `apply` passes the descriptor to the started process only.
    #[test]
    fn apply_passes_descriptors() {
        let parent = Loader::new_mock();
        assert_eq!(parent.dos_create_pipe(0x4000, 0x4004, 0), 0);
        let hr = parent.guest_read::<u32>(0x4000).unwrap();
        let hw = parent.guest_read::<u32>(0x4004).unwrap();
        parent.shared.handle_mgr.lock().unwrap().entry_mut(hr).unwrap().fh_state |= OPEN_FLAGS_NOINHERIT;
        let ch = parent.child_handles();
        let fd = ch.passed_fds()[0];
        let mut cmd = std::process::Command::new("sh");
        // Through /proc: dash, /bin/sh on Debian, takes only single-digit
        // descriptors in `>&N`, and with other tests running the duplicate
        // is often 10 or more.
        cmd.args(["-c", &format!("printf passed > /proc/self/fd/{fd}; echo \"${HANDLES_ENV}\"")]);
        cmd.stdout(std::process::Stdio::piped());
        ch.apply(&mut cmd);
        let out = cmd.output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with(&format!("{hw},{fd},a,")));
        drop(ch);
        assert_eq!(parent.dos_close(hw), 0);
        let f = parent.shared.handle_mgr.lock().unwrap().host_file(hr).unwrap();
        let mut got = String::new();
        (&*f).read_to_string(&mut got).unwrap();
        assert_eq!(got, "passed");
        // Without handles the variable is removed.
        let empty = ChildHandles { owned: vec![], list: vec![] };
        let mut cmd = std::process::Command::new("sh");
        cmd.env(HANDLES_ENV, "stale").args(["-c", &format!("printf \"${{{HANDLES_ENV}:-none}}\"")]);
        empty.apply(&mut cmd);
        assert_eq!(cmd.output().unwrap().stdout, b"none");
        let _ = std::io::stdout().flush();
    }
}
