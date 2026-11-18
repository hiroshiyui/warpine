// SPDX-License-Identifier: GPL-3.0-only
//
//! Session-wide thread accounting for `THREADS=` (`config_sys.rs`).
//!
//! Every OS/2 process is a Warpine host process of its own (ADR-0020), so
//! the threads of a session are counted in a private directory all its
//! processes share: `WARPINE_THREAD_DIR` (children get it from their parent;
//! the first process makes `$XDG_RUNTIME_DIR/warpine-threads-<uid>-<pid>`,
//! mode 0700, owned by the user — like the named-pipe directory).
//!
//! - `p-<pid>`: a live process; its record holds its pid, its start time
//!   (field 22 of `/proc/<pid>/stat`, so a reused pid is not mistaken for
//!   it) and its current number of threads.
//! - `r-<pid>-<seq>`: a slot process `<pid>` reserved for a child it is
//!   starting (record: the parent's pid and start time, 1 thread).
//!
//! All reading and writing happens under an exclusive `flock` on the
//! directory itself. A record counts only while its process lives (exists,
//! not a zombie, same start time); a stale or malformed record is removed
//! when it is seen, so a process killed by a signal leaks no slots.
//!
//! Starting a child: the parent reserves a slot (`reserve`) before it
//! spawns — two racing spawns cannot both pass —, passes the reservation's
//! name in `WARPINE_THREAD_SLOT`, then turns it into the child's `p-<pid>`
//! (`commit`) or drops it if the spawn failed (`cancel`). The child at its
//! start (`join`) finds its `p-<pid>` already written, or takes the
//! reservation over itself (whichever comes first, under the lock), or —
//! when neither exists — must find a free slot like a first process.
//!
//! Everything read from the directory is treated as hostile: names are
//! parsed strictly, records are at most `MAX_RECORD` bytes, regular files
//! opened without following links, and at most `MAX_ENTRIES` entries are
//! looked at (more means the session is full).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use log::warn;

use super::config_sys::MAX_THREADS;
use super::mutex_ext::MutexExt;

/// The session's accounting directory.
pub const THREAD_DIR_ENV: &str = "WARPINE_THREAD_DIR";
/// The reservation a child takes over.
pub const SLOT_ENV: &str = "WARPINE_THREAD_SLOT";
/// Largest record file.
const MAX_RECORD: usize = 64;
/// Most directory entries looked at.
const MAX_ENTRIES: usize = MAX_THREADS as usize + 64;
/// Largest pid Linux hands out (`PID_MAX_LIMIT`).
const PID_MAX: u32 = 4_194_304;
/// Largest `/proc/<pid>/stat` read.
const MAX_STAT: usize = 4096;
/// Attempts to lock a directory another process removes meanwhile.
const LOCK_ATTEMPTS: usize = 8;

/// A process: pid and start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub pid: u32,
    pub start: u64,
}

/// Start time of live process `pid` (`None`: no such process, a zombie,
/// or unreadable).
pub type StartTimeFn = Arc<dyn Fn(u32) -> Option<u64> + Send + Sync>;

/// The start time in a `/proc/<pid>/stat` line, `None` for a zombie or a
/// malformed line.
pub fn parse_stat(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    let mut fields = stat[close + 1..].split(u8::is_ascii_whitespace).filter(|f| !f.is_empty());
    let state = fields.next()?;
    if matches!(state, b"Z" | b"X" | b"x") { return None; }
    // Fields 4 … 22 of the line; the start time is the 22nd.
    let start = fields.nth(18)?;
    std::str::from_utf8(start).ok()?.parse().ok()
}

/// [`StartTimeFn`] from `/proc`.
pub fn proc_start_time(pid: u32) -> Option<u64> {
    if pid == 0 || pid > PID_MAX { return None; }
    let f = File::open(format!("/proc/{pid}/stat")).ok()?;
    let mut buf = Vec::with_capacity(512);
    f.take(MAX_STAT as u64).read_to_end(&mut buf).ok()?;
    parse_stat(&buf)
}

/// One record: whose, and how many threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    who: Identity,
    threads: u32,
}

impl Record {
    fn encode(&self) -> String {
        format!("{} {} {}\n", self.who.pid, self.who.start, self.threads)
    }

    fn decode(bytes: &[u8]) -> Option<Record> {
        if bytes.len() > MAX_RECORD { return None; }
        let text = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
        let mut parts = text.split(' ');
        fn num(s: Option<&str>) -> Option<&str> {
            s.filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
        }
        let pid: u32 = num(parts.next())?.parse().ok()?;
        let start: u64 = num(parts.next())?.parse().ok()?;
        let threads: u32 = num(parts.next())?.parse().ok()?;
        if parts.next().is_some() || pid == 0 || pid > PID_MAX || threads > MAX_THREADS {
            return None;
        }
        Some(Record { who: Identity { pid, start }, threads })
    }
}

/// What a directory entry's name says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Process(u32),
    Reservation(u32, u32),
}

fn parse_u32(s: &str) -> Option<u32> {
    (!s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok()).flatten()
}

fn parse_name(name: &OsStr) -> Option<Entry> {
    let name = name.to_str()?;
    if let Some(pid) = name.strip_prefix("p-") {
        return parse_u32(pid).map(Entry::Process);
    }
    let (pid, seq) = name.strip_prefix("r-")?.split_once('-')?;
    Some(Entry::Reservation(parse_u32(pid)?, parse_u32(seq)?))
}

fn process_name(pid: u32) -> String { format!("p-{pid}") }

/// The session directory, locked.
struct Locked {
    dir: File,
}

impl Locked {
    fn path(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{name}", self.dir.as_raw_fd()))
    }

    /// The record in `name`; `Ok(None)` when there is none.
    fn read(&self, name: &str) -> std::io::Result<Option<Option<Record>>> {
        let f = match OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(self.path(name)) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            // A link or something else odd: malformed.
            Err(_) => return Ok(Some(None)),
        };
        let meta = f.metadata()?;
        if !meta.is_file() || meta.len() > MAX_RECORD as u64 { return Ok(Some(None)); }
        let mut buf = Vec::with_capacity(MAX_RECORD);
        f.take(MAX_RECORD as u64 + 1).read_to_end(&mut buf)?;
        Ok(Some(Record::decode(&buf)))
    }

    fn write(&self, name: &str, rec: &Record) -> std::io::Result<()> {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW).open(self.path(name))?;
        f.write_all(rec.encode().as_bytes())
    }

    fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.path(name));
    }
}

/// Where the session directory is.
#[derive(Debug, Clone)]
pub struct SessionDir {
    path: PathBuf,
}

impl SessionDir {
    pub fn at(path: PathBuf) -> Self { SessionDir { path } }

    /// `WARPINE_THREAD_DIR`, else a new one for this process under
    /// `$XDG_RUNTIME_DIR` (or the temporary directory).
    pub fn from_env() -> Self {
        if let Some(p) = std::env::var_os(THREAD_DIR_ENV).filter(|p| !p.is_empty()) {
            return Self::at(PathBuf::from(p));
        }
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        Self::at(base.join(format!("warpine-threads-{}-{}", euid(), std::process::id())))
    }

    pub fn path(&self) -> &PathBuf { &self.path }

    /// Make (if needed), open and lock the directory: a real directory of
    /// this user, closed to everyone else. A directory removed by another
    /// process while this one waited for the lock is made again.
    fn lock(&self) -> std::io::Result<Locked> {
        for _ in 0..LOCK_ATTEMPTS {
            match std::fs::DirBuilder::new().mode(0o700).create(&self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            let dir = OpenOptions::new().read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(&self.path)?;
            let meta = dir.metadata()?;
            if !meta.is_dir() || meta.uid() != euid() || meta.mode() & 0o077 != 0 {
                return Err(std::io::Error::other("thread directory is not private"));
            }
            dir.lock()?;
            // Still the directory at the path (not removed meanwhile)?
            let now = dir.metadata()?;
            let same = std::fs::symlink_metadata(&self.path)
                .is_ok_and(|p| p.dev() == now.dev() && p.ino() == now.ino());
            if now.nlink() > 0 && same {
                return Ok(Locked { dir });
            }
        }
        Err(std::io::Error::other("thread directory keeps disappearing"))
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Why a slot cannot be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Full {
    /// The session runs `limit` threads already.
    Limit,
}

/// A slot reserved for a child being started.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a reservation is committed or cancelled"]
pub struct Reservation {
    name: Option<String>,
}

impl Reservation {
    /// The name the child gets in `WARPINE_THREAD_SLOT` (`None`: the
    /// session is not shared).
    pub fn name(&self) -> Option<&str> { self.name.as_deref() }
}

/// This process's part of the session.
pub struct ThreadSession {
    dir: SessionDir,
    limit: u32,
    me: Identity,
    /// Threads this process runs (as recorded).
    threads: u32,
    next_seq: u32,
    start_time: StartTimeFn,
    /// The directory cannot be used: only this process's threads count.
    local_only: bool,
    /// The process left the session (it is ending): nothing more is
    /// written, so a thread ending meanwhile cannot bring the record back.
    left: bool,
}

impl ThreadSession {
    /// A session at `dir` with `limit` threads for process `me`.
    pub fn new(dir: SessionDir, limit: u32, me: Identity, start_time: StartTimeFn) -> Self {
        ThreadSession { dir, limit, me, threads: 0, next_seq: 0, start_time, local_only: false, left: false }
    }

    /// This host process in the session of the environment.
    pub fn for_this_process(limit: u32) -> Self {
        let pid = std::process::id();
        let start = proc_start_time(pid).unwrap_or(0);
        Self::new(SessionDir::from_env(), limit, Identity { pid, start }, Arc::new(proc_start_time))
    }

    pub fn limit(&self) -> u32 { self.limit }
    pub fn threads(&self) -> u32 { self.threads }
    pub fn dir(&self) -> &SessionDir { &self.dir }

    fn is_alive(&self, who: Identity) -> bool {
        (self.start_time)(who.pid) == Some(who.start)
    }

    /// Threads counted by every live record except this process's own
    /// `p-` file; stale and malformed records are removed. More than
    /// `MAX_ENTRIES` entries count as a full session.
    fn others(&self, l: &Locked) -> std::io::Result<u64> {
        let own = process_name(self.me.pid);
        let mut total = 0u64;
        let mut seen = HashSet::new();
        for (i, entry) in std::fs::read_dir(l.path(""))?.enumerate() {
            if i >= MAX_ENTRIES { return Ok(u64::from(self.limit)); }
            let entry = entry?;
            let file = entry.file_name();
            let Some(kind) = parse_name(&file) else { continue };
            let Some(name) = file.to_str() else { continue };
            if name == own || !seen.insert(name.to_owned()) { continue; }
            let owner = match kind { Entry::Process(p) | Entry::Reservation(p, _) => p };
            match l.read(name)? {
                None => {}
                Some(Some(rec)) if rec.who.pid == owner && self.is_alive(rec.who) => {
                    total = total.saturating_add(u64::from(rec.threads));
                }
                Some(_) => l.remove(name),
            }
        }
        Ok(total)
    }

    fn own_record(&self) -> Record { Record { who: self.me, threads: self.threads } }

    fn fall_back(&mut self, what: &str, e: &std::io::Error) {
        if !self.local_only {
            warn!("thread accounting: {what}: {e}; counting this process's threads only");
            self.local_only = true;
        }
    }

    /// Enter the session as a process with thread 1: take over the slot
    /// the parent reserved (`slot`, `WARPINE_THREAD_SLOT`) or a free one.
    pub fn join(&mut self, slot: Option<&str>) -> Result<(), Full> {
        match self.try_join(slot) {
            Ok(r) => r,
            Err(e) => {
                self.fall_back("cannot join the session", &e);
                self.threads = 1;
                Ok(())
            }
        }
    }

    fn try_join(&mut self, slot: Option<&str>) -> std::io::Result<Result<(), Full>> {
        let l = self.dir.lock()?;
        let own = process_name(self.me.pid);
        let mine = matches!(l.read(&own)?, Some(Some(rec)) if rec.who == self.me);
        let reserved = !mine && match slot.and_then(|s| parse_name(OsStr::new(s)).map(|k| (s, k))) {
            Some((s, Entry::Reservation(owner, _))) => match l.read(s)? {
                Some(Some(rec)) if rec.who.pid == owner && self.is_alive(rec.who) => {
                    l.remove(s);
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !mine && !reserved && self.others(&l)?.saturating_add(1) > u64::from(self.limit) {
            return Ok(Err(Full::Limit));
        }
        self.threads = 1;
        l.write(&own, &self.own_record())?;
        Ok(Ok(()))
    }

    /// A slot for one more thread of this process.
    pub fn acquire_thread(&mut self) -> Result<(), Full> {
        if self.left { return Err(Full::Limit); }
        if self.local_only { return self.acquire_local(); }
        let r = (|| {
            let l = self.dir.lock()?;
            let used = self.others(&l)?.saturating_add(u64::from(self.threads));
            if used.saturating_add(1) > u64::from(self.limit) { return Ok(Err(Full::Limit)); }
            self.threads += 1;
            if let Err(e) = l.write(&process_name(self.me.pid), &self.own_record()) {
                self.threads -= 1;
                return Err(e);
            }
            Ok(Ok(()))
        })();
        r.unwrap_or_else(|e| { self.fall_back("cannot count threads", &e); self.acquire_local() })
    }

    fn acquire_local(&mut self) -> Result<(), Full> {
        if self.threads >= self.limit { return Err(Full::Limit); }
        self.threads += 1;
        Ok(())
    }

    /// One thread of this process ended.
    pub fn release_thread(&mut self) {
        self.threads = self.threads.saturating_sub(1);
        if self.local_only || self.left { return; }
        if let Ok(l) = self.dir.lock() {
            let _ = l.write(&process_name(self.me.pid), &self.own_record());
        }
    }

    /// Reserve a slot for thread 1 of a child about to be started.
    pub fn reserve(&mut self) -> Result<Reservation, Full> {
        if self.left { return Err(Full::Limit); }
        if self.local_only { return Ok(Reservation { name: None }); }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let name = format!("r-{}-{seq}", self.me.pid);
        let r = (|| {
            let l = self.dir.lock()?;
            let used = self.others(&l)?.saturating_add(u64::from(self.threads));
            if used.saturating_add(1) > u64::from(self.limit) { return Ok(Err(Full::Limit)); }
            l.write(&name, &Record { who: self.me, threads: 1 })?;
            Ok(Ok(()))
        })();
        match r {
            Ok(Ok(())) => Ok(Reservation { name: Some(name) }),
            Ok(Err(f)) => Err(f),
            Err(e) => {
                self.fall_back("cannot reserve a slot", &e);
                Ok(Reservation { name: None })
            }
        }
    }

    /// The child of `res` started as host process `child` (`None`: the
    /// spawn failed): its slot becomes its `p-` record — unless it already
    /// took the reservation over — or is given back.
    pub fn settle(&mut self, res: Reservation, child: Option<u32>) {
        let Some(name) = res.name else { return };
        if self.left { return; }
        let Ok(l) = self.dir.lock() else { return };
        if !matches!(l.read(&name), Ok(Some(_))) { return; }
        l.remove(&name);
        let Some(pid) = child.filter(|&p| p != 0 && p <= PID_MAX) else { return };
        if let Some(start) = (self.start_time)(pid) {
            let own = process_name(pid);
            if !matches!(l.read(&own), Ok(Some(Some(_)))) {
                let _ = l.write(&own, &Record { who: Identity { pid, start }, threads: 1 });
            }
        }
    }

    /// The process ends: its record goes, and the directory too once empty.
    pub fn leave(&mut self) {
        self.threads = 0;
        if self.local_only || std::mem::replace(&mut self.left, true) { return; }
        if let Ok(l) = self.dir.lock() {
            l.remove(&process_name(self.me.pid));
            // Records of processes that were killed go too (a scan removes
            // them), then the directory — which fails, harmlessly, while
            // live records are there.
            let _ = self.others(&l);
            let _ = std::fs::remove_dir(&self.dir.path);
        }
    }
}

/// The process's thread accounting: inactive (unit tests' loaders) or a
/// session.
#[derive(Default)]
pub enum ThreadSlots {
    #[default]
    Inactive,
    Session(Box<ThreadSession>),
}

impl ThreadSlots {
    pub fn acquire_thread(&mut self) -> Result<(), Full> {
        match self { ThreadSlots::Inactive => Ok(()), ThreadSlots::Session(s) => s.acquire_thread() }
    }

    pub fn release_thread(&mut self) {
        if let ThreadSlots::Session(s) = self { s.release_thread(); }
    }

    /// Reserve a child's slot and pass the session to `cmd`.
    pub fn reserve_for(&mut self, cmd: &mut std::process::Command) -> Result<Reservation, Full> {
        let ThreadSlots::Session(s) = self else {
            cmd.env_remove(SLOT_ENV);
            return Ok(Reservation { name: None });
        };
        let res = s.reserve()?;
        cmd.env(THREAD_DIR_ENV, s.dir().path());
        cmd.env(super::config_sys::THREADS_ENV, s.limit().to_string());
        match res.name() {
            Some(n) => { cmd.env(SLOT_ENV, n); }
            None => { cmd.env_remove(SLOT_ENV); }
        }
        Ok(res)
    }

    pub fn settle(&mut self, res: Reservation, child: Option<u32>) {
        if let ThreadSlots::Session(s) = self { s.settle(res, child); }
    }

    pub fn leave(&mut self) {
        if let ThreadSlots::Session(s) = self { s.leave(); }
    }
}

/// Exit code of a Warpine process that finds no thread slot at its start
/// (`ERROR_NO_PROC_SLOTS`).
pub const EXIT_NO_THREAD_SLOT: i32 = 89;

impl super::Loader {
    /// Enter the session's thread accounting with thread 1 (limit from
    /// `WARPINE_THREADS` or drive C's CONFIG.SYS, `config_sys.rs`); `false`
    /// when the session has no slot left — the process must then end with
    /// [`EXIT_NO_THREAD_SLOT`] before running guest code.
    pub fn join_thread_session(&self) -> bool {
        let limit = super::config_sys::session_limit(&self.shared.drive_mgr.lock_or_recover());
        let mut session = ThreadSession::for_this_process(limit);
        let slot = std::env::var(SLOT_ENV).ok();
        if session.join(slot.as_deref()).is_err() {
            return false;
        }
        log::debug!("thread accounting: limit {limit} in {}", session.dir().path().display());
        *self.shared.thread_slots.lock_or_recover() = ThreadSlots::Session(Box::new(session));
        true
    }
}

impl super::SharedState {
    /// This process leaves the session's thread accounting (it ends).
    pub fn leave_thread_session(&self) {
        self.thread_slots.lock_or_recover().leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Simulated host processes: pid → start time while alive.
    #[derive(Clone, Default)]
    struct Procs(Arc<Mutex<HashMap<u32, u64>>>);

    impl Procs {
        fn spawn(&self, pid: u32) -> Identity {
            let start = 1000 + u64::from(pid);
            self.0.lock().unwrap().insert(pid, start);
            Identity { pid, start }
        }
        fn kill(&self, pid: u32) { self.0.lock().unwrap().remove(&pid); }
        fn reuse(&self, pid: u32) { self.0.lock().unwrap().insert(pid, 99); }
        fn f(&self) -> StartTimeFn {
            let m = self.0.clone();
            Arc::new(move |pid| m.lock().unwrap().get(&pid).copied())
        }
        fn session(&self, dir: &std::path::Path, limit: u32, pid: u32) -> ThreadSession {
            let me = self.spawn(pid);
            ThreadSession::new(SessionDir::at(dir.join("s")), limit, me, self.f())
        }
    }

    fn files(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir.join("s")).map(|r| r.flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn stat_lines_give_start_time_unless_zombie() {
        let line = b"42 (a) b) c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 20\n";
        assert_eq!(parse_stat(line), Some(777));
        let zombie = b"42 (x) Z 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 20\n";
        assert_eq!(parse_stat(zombie), None);
        assert_eq!(parse_stat(b"42 (x) S 1 2"), None);
        assert_eq!(parse_stat(b"garbage"), None);
        let me = std::process::id();
        assert!(proc_start_time(me).is_some());
        assert_eq!(proc_start_time(0), None);
        assert_eq!(proc_start_time(PID_MAX + 1), None);
    }

    #[test]
    fn records_and_names_are_parsed_strictly() {
        let r = Record { who: Identity { pid: 5, start: 7 }, threads: 3 };
        assert_eq!(Record::decode(r.encode().as_bytes()), Some(r));
        for bad in [&b"5 7 3"[..], b"5 7 3 1\n", b"0 7 3\n", b"5 7 99999\n", b"-5 7 3\n",
                    b"5  7 3\n", b"5 7 +3\n", &[b'1'; 80][..], b"\xff 7 3\n"] {
            assert_eq!(Record::decode(bad), None, "{bad:?}");
        }
        assert_eq!(parse_name(OsStr::new("p-12")), Some(Entry::Process(12)));
        assert_eq!(parse_name(OsStr::new("r-12-3")), Some(Entry::Reservation(12, 3)));
        for bad in ["p-", "p-x", "p-1-2", "r-1", "r-1-", "q-1", "p-+1", "p-12345678901", "r-1-2-3"] {
            assert_eq!(parse_name(OsStr::new(bad)), None, "{bad}");
        }
    }

    #[test]
    fn processes_share_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        let mut a = procs.session(d.path(), 5, 100);
        let mut b = procs.session(d.path(), 5, 200);
        a.join(None).unwrap();
        b.join(None).unwrap();
        assert_eq!(a.acquire_thread(), Ok(()));
        assert_eq!(b.acquire_thread(), Ok(()));
        assert_eq!(a.acquire_thread(), Ok(()));
        // 3 + 2 = 5 threads: full for both.
        assert_eq!(b.acquire_thread(), Err(Full::Limit));
        assert_eq!(a.acquire_thread(), Err(Full::Limit));
        let mut c = procs.session(d.path(), 5, 300);
        assert_eq!(c.join(None), Err(Full::Limit));
        a.release_thread();
        assert_eq!(b.acquire_thread(), Ok(()));
        assert_eq!(files(d.path()), ["p-100", "p-200"]);
        a.leave();
        // A thread ending while the process ends writes nothing more.
        a.release_thread();
        assert_eq!(a.acquire_thread(), Err(Full::Limit));
        assert_eq!(files(d.path()), ["p-200"]);
        b.leave();
        // The last one out removes the directory.
        assert!(!d.path().join("s").exists());
        b.release_thread();
        assert!(!d.path().join("s").exists());
    }

    #[test]
    fn dead_processes_and_junk_are_cleaned_up() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        let mut a = procs.session(d.path(), 4, 100);
        let mut b = procs.session(d.path(), 4, 200);
        let mut c = procs.session(d.path(), 4, 300);
        a.join(None).unwrap();
        b.join(None).unwrap();
        b.acquire_thread().unwrap();
        b.acquire_thread().unwrap();
        c.join(None).unwrap_err();
        // b killed by a signal: its slots come back.
        procs.kill(200);
        c.join(None).unwrap();
        // A reused pid is not the old process.
        let mut e = procs.session(d.path(), 4, 400);
        e.join(None).unwrap();
        procs.reuse(400);
        let s = d.path().join("s");
        std::fs::write(s.join("p-555"), b"junk").unwrap();
        std::fs::write(s.join("p-556"), b"557 1556 1\n").unwrap(); // pid mismatch
        std::fs::write(s.join("other"), b"ignored").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", s.join("p-558")).unwrap();
        assert_eq!(a.acquire_thread(), Ok(()));
        assert_eq!(a.acquire_thread(), Ok(()));
        assert_eq!(files(d.path()), ["other", "p-100", "p-300"]);
        // The last live process removes a killed one's record on leaving.
        std::fs::remove_file(d.path().join("s/other")).unwrap();
        procs.kill(300);
        a.leave();
        assert!(!d.path().join("s").exists());
    }

    #[test]
    fn reservations_count_and_are_taken_over() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        let mut parent = procs.session(d.path(), 3, 100);
        parent.join(None).unwrap();
        let r1 = parent.reserve().unwrap();
        let r2 = parent.reserve().unwrap();
        assert_eq!(parent.reserve(), Err(Full::Limit));
        assert_eq!(parent.acquire_thread(), Err(Full::Limit));
        // Child 1 starts before its parent settles: takes the reservation.
        let mut c1 = procs.session(d.path(), 3, 201);
        assert_eq!(c1.join(r1.name()), Ok(()));
        parent.settle(r1, Some(201));
        // Child 2: the parent settles first, the child confirms.
        procs.spawn(202);
        parent.settle(r2, Some(202));
        let mut c2 = procs.session(d.path(), 3, 202);
        assert_eq!(c2.join(Some("r-100-1")), Ok(()));
        assert_eq!(files(d.path()), ["p-100", "p-201", "p-202"]);
        // A failed spawn gives the slot back.
        c2.leave();
        let r3 = parent.reserve().unwrap();
        parent.settle(r3, None);
        assert_eq!(files(d.path()), ["p-100", "p-201"]);
        // A slot name of another (dead) process does not help.
        let mut c3 = procs.session(d.path(), 3, 203);
        std::fs::write(d.path().join("s/r-999-0"), b"999 1999 1\n").unwrap();
        assert_eq!(c3.join(Some("r-999-0")), Ok(()));
        let mut c4 = procs.session(d.path(), 3, 204);
        assert_eq!(c4.join(Some("r-999-0")), Err(Full::Limit));
    }

    #[test]
    fn reservations_of_a_dead_parent_are_stale() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        let mut parent = procs.session(d.path(), 2, 100);
        parent.join(None).unwrap();
        let _r = parent.reserve().unwrap();
        procs.kill(100);
        let mut other = procs.session(d.path(), 2, 300);
        other.join(None).unwrap();
        assert_eq!(other.acquire_thread(), Ok(()));
        assert_eq!(files(d.path()), ["p-300"]);
    }

    #[test]
    fn racing_reservations_never_exceed_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        const LIMIT: u32 = 10;
        let mut handles = Vec::new();
        for pid in 1..=8u32 {
            let mut s = procs.session(d.path(), LIMIT, pid * 10);
            handles.push(std::thread::spawn(move || {
                let mut got = 0;
                if s.join(None).is_ok() {
                    got += 1;
                    for _ in 0..4 {
                        if let Ok(r) = s.reserve() { got += 1; std::mem::forget(r); }
                    }
                }
                got
            }));
        }
        let total: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, LIMIT);
    }

    #[test]
    fn unusable_directory_counts_locally() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("s"), b"a file").unwrap();
        let procs = Procs::default();
        let mut a = procs.session(d.path(), 3, 100);
        assert_eq!(a.join(None), Ok(()));
        assert_eq!(a.reserve().map(|r| r.name().is_none()), Ok(true));
        assert_eq!(a.acquire_thread(), Ok(()));
        assert_eq!(a.acquire_thread(), Ok(()));
        assert_eq!(a.acquire_thread(), Err(Full::Limit));
    }

    #[test]
    fn a_shared_directory_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join("s");
        std::fs::create_dir(&s).unwrap();
        std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(SessionDir::at(s.clone()).lock().is_err());
        std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(SessionDir::at(s).lock().is_ok());
    }

    #[test]
    fn slots_pass_the_session_to_a_child() {
        let d = tempfile::tempdir().unwrap();
        let procs = Procs::default();
        let mut s = procs.session(d.path(), 70, 100);
        s.join(None).unwrap();
        let mut slots = ThreadSlots::Session(Box::new(s));
        let mut cmd = std::process::Command::new("true");
        let r = slots.reserve_for(&mut cmd).unwrap();
        let envs: HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(envs[OsStr::new(SLOT_ENV)], Some(OsStr::new("r-100-0")));
        assert_eq!(envs[OsStr::new(super::super::config_sys::THREADS_ENV)], Some(OsStr::new("70")));
        assert_eq!(envs[OsStr::new(THREAD_DIR_ENV)], Some(d.path().join("s").as_os_str()));
        slots.settle(r, None);
        let mut inactive = ThreadSlots::Inactive;
        assert_eq!(inactive.reserve_for(&mut cmd).map(|r| r.name().is_none()), Ok(true));
        assert_eq!(inactive.acquire_thread(), Ok(()));
    }

    /// A loader whose session (in `dir`) allows `limit` threads and runs
    /// thread 1 already.
    fn loader_in_session(dir: &std::path::Path, limit: u32) -> super::super::Loader {
        let procs = Procs::default();
        let mut s = procs.session(dir, limit, 4242);
        s.join(None).unwrap();
        let l = super::super::Loader::new_mock();
        *l.shared.thread_slots.lock().unwrap() = ThreadSlots::Session(Box::new(s));
        l
    }

    #[test]
    fn dos_create_thread_fails_at_the_limit() {
        use super::super::constants::ERROR_MAX_THRDS_REACHED;
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path(), 1);
        let free_before = l.shared.mem_mgr.lock().unwrap().alloc(0x1000).unwrap();
        l.shared.mem_mgr.lock().unwrap().free(free_before);
        assert_eq!(l.dos_create_thread(0, 0x7_0000, 0x1_0000, 0, 0, 0x2000), ERROR_MAX_THRDS_REACHED);
        // Its stack and TIB were given back.
        assert_eq!(l.shared.mem_mgr.lock().unwrap().alloc(0x1000), Some(free_before));
        if let ThreadSlots::Session(s) = &*l.shared.thread_slots.lock().unwrap() {
            assert_eq!(s.threads(), 1);
        }
    }

    #[test]
    fn children_need_a_slot() {
        use super::super::inherit::{admit_child, child_started, ChildLimit};
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path(), 2);
        let mut cmd = std::process::Command::new("true");
        let r = admit_child(&l.shared, &mut cmd);
        // At the depth limit already (a test run inside a Warpine child)?
        if super::super::inherit::own_depth() >= super::super::inherit::MAX_PROCESS_DEPTH { return; }
        let r = r.unwrap();
        let mut cmd2 = std::process::Command::new("true");
        assert_eq!(admit_child(&l.shared, &mut cmd2).err(), Some(ChildLimit::Threads));
        child_started(&l.shared, r, None);
        let r = admit_child(&l.shared, &mut cmd2).unwrap();
        child_started(&l.shared, r, None);
    }
}
