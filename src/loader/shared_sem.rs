// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Shared semaphores across the Warpine processes of a session (ADR-0050).
//!
//! Event, mutex and muxwait semaphores that are named (`\SEM32\…`) or
//! created with `DC_SEM_SHARED` live in a segment all processes of a session map: a
//! sealed `memfd` of `SEGMENT_BYTES` (the first process makes it, children
//! inherit it through `WARPINE_SEM_FD`, as the shared arena,
//! `shared_arena.rs`). Unlike the arena it is **not** in guest memory: only
//! Warpine's host code reads and writes it.
//!
//! - The segment is a header (magic, version, the generation word) and
//!   `MAX_SHARED_SEMS` fixed slots. A slot holds one semaphore: kind,
//!   generation, flags, name, event post count, mutex owner or muxwait
//!   list, and the processes that opened it with their counts.
//! - A shared semaphore's handle is `SHARED_SEM_BIT | generation_no << 16 | slot`, the
//!   same value in every process (a handle passed to another process opens
//!   it there). Freeing a slot moves its generation on, so an old handle
//!   does not reach a new semaphore.
//! - Every look and change happens under one lock: an in-process mutex and
//!   an open-file-description lock (`F_OFD_SETLKW`) on this process's own
//!   description of the memfd (opened through `/proc/self/fd`). The kernel
//!   drops the lock of a process that dies, so a killed process cannot
//!   leave the segment locked.
//! - Waiters sleep on the header's generation word with a non-private futex;
//!   every change moves it on and wakes them all, in every process, and
//!   they look again (and every `SEM_POLL` anyway, for their stop
//!   conditions).
//! - A process is identified by its OS/2 id and its host identity (pid and
//!   start time). A mutex whose owning process is gone is in the
//!   owner-died state; openers that are gone are dropped; a semaphore no
//!   live process has open is deleted. The ending process does that for
//!   itself; a killed one is noticed by the next process that looks.
//!
//! Everything read from a slot is checked: kinds, lengths and counts are
//! bounded, names must be `\SEM32\` names; a slot that does not parse is
//! free. A process of the same user could still write nonsense into the
//! segment; that can make semaphores misbehave but never makes Warpine
//! read or write outside it.
//!
//! A shared muxwait list holds shared events or mutexes only, by handle; a
//! member is looked up whoever has it open, so a process that opened the
//! list waits on members it never opened itself (Warpine's reading,
//! unverified). A private muxwait list can hold shared semaphores too
//! (`ipc.rs`).

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use log::warn;

use super::constants::*;
use super::ipc::{check_mux_attr, check_mux_records, EventSemaphore, MuxWaitRecord, SemKind};
use super::mutex_ext::MutexExt;
use super::session_threads::Identity;

/// The descriptor of the session's semaphore segment a child inherits.
pub const SEM_FD_ENV: &str = "WARPINE_SEM_FD";
/// The bit that marks a shared semaphore's handle.
pub const SHARED_SEM_BIT: u32 = 0x8000_0000;
/// Semaphores the segment holds.
pub const MAX_SHARED_SEMS: usize = 512;
/// Processes one semaphore records as having it open.
pub const MAX_OPENERS: usize = 28;
const SLOT_BYTES: usize = 2048;
const HEADER_BYTES: usize = 4096;
pub const SEGMENT_BYTES: usize = HEADER_BYTES + MAX_SHARED_SEMS * SLOT_BYTES;
const MAGIC: u32 = 0x4D45_5357; // "WSEM"
/// 2: 2 KB slots with muxwait lists.
const VERSION: u32 = 2;
/// Offset of the generation word in the header.
const GENERATION_AT: usize = 8;
/// The longest semaphore name, `\SEM32\` included.
const MAX_NAME: usize = 255;
const NAME_AT: usize = 56;
const OPENERS_AT: usize = NAME_AT + 256;
const OPENER_BYTES: usize = 24;
const _: () = assert!(OPENERS_AT + MAX_OPENERS * OPENER_BYTES <= RECORDS_AT);
/// A muxwait list's `SEMRECORD`s: handle and user value.
const RECORDS_AT: usize = 1024;
const RECORD_BYTES: usize = 8;
const _: () = assert!(RECORDS_AT + MAX_MUXWAIT_SEMS * RECORD_BYTES <= SLOT_BYTES);
const KIND_FREE: u32 = 0;
const KIND_EVENT: u32 = 1;
const KIND_MUTEX: u32 = 2;
const KIND_MUXWAIT: u32 = 3;

/// A process of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proc {
    pub os2pid: u32,
    pub host: Identity,
}

/// A mutex owner: the process and its thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    pub proc: Proc,
    pub tid: u32,
}

/// What a semaphore is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Event { post_count: u32 },
    Mutex {
        owner: Option<Owner>,
        /// Requests less releases by the owner.
        count: u32,
        /// The owner ended without releasing it: its OS/2 process and thread.
        died: Option<(u32, u32)>,
    },
    /// A muxwait list of shared semaphores of one kind (`None` while it is
    /// empty); `DCMW_WAIT_ALL` is in the slot's flags.
    MuxWait { records: Vec<MuxWaitRecord>, list: Option<SemKind> },
}

/// One semaphore of the segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub generation_no: u32,
    pub attr: u32,
    pub name: Option<String>,
    pub state: State,
    /// Who has it open, and how often.
    pub openers: Vec<(Proc, u32)>,
}

fn get_u32(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) }
fn get_u64(b: &[u8], at: usize) -> u64 { u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) }
fn put_u32(b: &mut [u8], at: usize, v: u32) { b[at..at + 4].copy_from_slice(&v.to_le_bytes()); }
fn put_u64(b: &mut [u8], at: usize, v: u64) { b[at..at + 8].copy_from_slice(&v.to_le_bytes()); }

/// Whether `name` is an upper-case `\SEM32\…` name within the bounds.
pub fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME && name.len() > SEM32_PREFIX.len() && name.starts_with(SEM32_PREFIX)
        && name.to_ascii_uppercase() == name && !name.chars().any(char::is_control)
}

impl Slot {
    /// The slot in `b` (`None`: free or malformed).
    pub fn decode(b: &[u8; SLOT_BYTES]) -> Option<Slot> {
        let kind = get_u32(b, 0);
        let generation_no = get_u32(b, 4);
        let attr = get_u32(b, 8);
        let state = match kind {
            KIND_EVENT => {
                let post_count = get_u32(b, 12);
                if post_count > MAX_EVENT_POSTS { return None; }
                State::Event { post_count }
            }
            KIND_MUTEX => {
                let (pid, tid, host_pid, count, start) =
                    (get_u32(b, 16), get_u32(b, 20), get_u32(b, 24), get_u32(b, 28), get_u64(b, 32));
                let owner = (pid != 0).then_some(Owner {
                    proc: Proc { os2pid: pid, host: Identity { pid: host_pid, start } }, tid,
                });
                let died = (get_u32(b, 40) != 0).then(|| (get_u32(b, 40), get_u32(b, 44)));
                if owner.is_some() != (count > 0) || (owner.is_some() && died.is_some()) { return None; }
                State::Mutex { owner, count, died }
            }
            KIND_MUXWAIT => {
                check_mux_attr(attr).ok()?;
                let n = get_u32(b, 12) as usize;
                let list = match get_u32(b, 16) {
                    0 => None,
                    KIND_EVENT => Some(SemKind::Event),
                    KIND_MUTEX => Some(SemKind::Mutex),
                    _ => return None,
                };
                if n > MAX_MUXWAIT_SEMS || (n == 0) != list.is_none() { return None; }
                let records: Vec<MuxWaitRecord> = (0..n).map(|i| {
                    let at = RECORDS_AT + i * RECORD_BYTES;
                    MuxWaitRecord { hsem: get_u32(b, at), user: get_u32(b, at + 4) }
                }).collect();
                let bad = |(i, r): (usize, &MuxWaitRecord)| slot_of(r.hsem).is_none()
                    || records[..i].iter().any(|o| o.hsem == r.hsem);
                if records.iter().enumerate().any(bad) { return None; }
                State::MuxWait { records, list }
            }
            _ => return None,
        };
        let name_len = get_u32(b, 48) as usize;
        let name = if name_len == 0 { None } else {
            if name_len > MAX_NAME { return None; }
            let n = std::str::from_utf8(&b[NAME_AT..NAME_AT + name_len]).ok()?;
            if !valid_name(n) { return None; }
            Some(n.to_string())
        };
        let n = get_u32(b, 52) as usize;
        if n == 0 || n > MAX_OPENERS { return None; }
        let mut openers = Vec::with_capacity(n);
        for i in 0..n {
            let at = OPENERS_AT + i * OPENER_BYTES;
            let proc = Proc {
                os2pid: get_u32(b, at),
                host: Identity { pid: get_u32(b, at + 4), start: get_u64(b, at + 8) },
            };
            let count = get_u32(b, at + 16);
            if count == 0 || count > MAX_SEM_OPENS || openers.iter().any(|(p, _): &(Proc, u32)| p.os2pid == proc.os2pid) {
                return None;
            }
            openers.push((proc, count));
        }
        Some(Slot { generation_no, attr, name, state, openers })
    }

    pub fn encode(&self) -> [u8; SLOT_BYTES] {
        let mut b = [0u8; SLOT_BYTES];
        put_u32(&mut b, 4, self.generation_no);
        put_u32(&mut b, 8, self.attr);
        match &self.state {
            State::Event { post_count } => {
                put_u32(&mut b, 0, KIND_EVENT);
                put_u32(&mut b, 12, *post_count);
            }
            State::Mutex { owner, count, died } => {
                put_u32(&mut b, 0, KIND_MUTEX);
                if let Some(o) = owner {
                    put_u32(&mut b, 16, o.proc.os2pid);
                    put_u32(&mut b, 20, o.tid);
                    put_u32(&mut b, 24, o.proc.host.pid);
                    put_u64(&mut b, 32, o.proc.host.start);
                }
                put_u32(&mut b, 28, *count);
                if let Some((pid, tid)) = died {
                    put_u32(&mut b, 40, *pid);
                    put_u32(&mut b, 44, *tid);
                }
            }
            State::MuxWait { records, list } => {
                put_u32(&mut b, 0, KIND_MUXWAIT);
                put_u32(&mut b, 12, records.len() as u32);
                put_u32(&mut b, 16, match list {
                    Some(SemKind::Event) => KIND_EVENT,
                    Some(SemKind::Mutex) => KIND_MUTEX,
                    _ => 0,
                });
                for (i, r) in records.iter().enumerate() {
                    put_u32(&mut b, RECORDS_AT + i * RECORD_BYTES, r.hsem);
                    put_u32(&mut b, RECORDS_AT + i * RECORD_BYTES + 4, r.user);
                }
            }
        }
        if let Some(n) = &self.name {
            put_u32(&mut b, 48, n.len() as u32);
            b[NAME_AT..NAME_AT + n.len()].copy_from_slice(n.as_bytes());
        }
        put_u32(&mut b, 52, self.openers.len() as u32);
        for (i, (p, count)) in self.openers.iter().enumerate() {
            let at = OPENERS_AT + i * OPENER_BYTES;
            put_u32(&mut b, at, p.os2pid);
            put_u32(&mut b, at + 4, p.host.pid);
            put_u64(&mut b, at + 8, p.host.start);
            put_u32(&mut b, at + 16, *count);
        }
        b
    }

    pub fn kind(&self) -> SemKind {
        match self.state {
            State::Event { .. } => SemKind::Event,
            State::Mutex { .. } => SemKind::Mutex,
            State::MuxWait { .. } => SemKind::MuxWait,
        }
    }

    fn opens_of(&self, os2pid: u32) -> u32 {
        self.openers.iter().find(|(p, _)| p.os2pid == os2pid).map_or(0, |&(_, c)| c)
    }

    /// Drop openers that are not alive and mark a mutex whose owner is not
    /// alive as owner-died. False when no opener is left.
    pub fn sweep(&mut self, alive: &dyn Fn(Identity) -> bool) -> bool {
        self.openers.retain(|(p, _)| alive(p.host));
        if let State::Mutex { owner, count, died } = &mut self.state
            && let Some(o) = *owner && !alive(o.proc.host) {
            *died = Some((o.proc.os2pid, o.tid));
            *owner = None;
            *count = 0;
        }
        !self.openers.is_empty()
    }

    /// The event's post / reset / consume logic (`ipc::EventSemaphore`).
    fn with_event<R>(&mut self, f: impl FnOnce(&mut EventSemaphore) -> R) -> Option<R> {
        let State::Event { post_count } = &mut self.state else { return None };
        let mut e = EventSemaphore::new(false, self.attr);
        e.post_count = *post_count;
        let r = f(&mut e);
        *post_count = e.post_count;
        Some(r)
    }

    /// Whether thread `who` can take the mutex without waiting.
    pub fn mutex_available(&self, who: Owner) -> bool {
        match &self.state {
            State::Mutex { owner, died, .. } => died.is_some() || owner.is_none_or(|o| o == who),
            State::Event { .. } | State::MuxWait { .. } => false,
        }
    }

    /// Take (or take again) the mutex for `who`, which must be available.
    /// True when its last owner had died.
    pub fn mutex_acquire(&mut self, who: Owner) -> bool {
        let State::Mutex { owner, count, died } = &mut self.state else { return false };
        if died.take().is_some() {
            *owner = Some(who);
            *count = 1;
            return true;
        }
        if *owner != Some(who) {
            *owner = Some(who);
            *count = 1;
            return false;
        }
        *count += 1;
        false
    }
}

/// The kinds a slot can hold.
const ALL_KINDS: [SemKind; 3] = [SemKind::Event, SemKind::Mutex, SemKind::MuxWait];

/// The handle of slot `i` with generation `generation_no`.
pub fn handle(i: usize, generation_no: u32) -> u32 {
    SHARED_SEM_BIT | ((generation_no & 0x7FFF) << 16) | i as u32
}

/// Whether `h` is a shared semaphore's handle.
pub fn is_shared(h: u32) -> bool { h & SHARED_SEM_BIT != 0 }

/// The slot index of `h`, when it could be one.
fn slot_of(h: u32) -> Option<usize> {
    let i = (h & 0xFFFF) as usize;
    (is_shared(h) && i < MAX_SHARED_SEMS).then_some(i)
}

// ── The segment ──────────────────────────────────────────────────────────

/// The seals the segment's memfd carries.
const SEALS: libc::c_int = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;

/// This process's view of the session's semaphore segment.
pub struct SharedSems {
    base: *mut u8,
    fd: OwnedFd,
    /// This process's own open file description, for the OFD lock.
    lock_fd: OwnedFd,
    /// Threads of this process: one at a time under the OFD lock.
    exclusive: Mutex<()>,
    /// Threads of this process blocked on each shared semaphore
    /// (`ERROR_SEM_BUSY` is about this process's threads).
    waiters: Mutex<HashMap<u32, u32>>,
    me: Proc,
    alive: Box<dyn Fn(Identity) -> bool + Send + Sync>,
}

// SAFETY: `base` points at a shared mapping of `SEGMENT_BYTES` that lives as
// long as the value (unmapped in Drop). It is only read and written through
// bounds-checked copies under the lock, and the generation word through an
// `AtomicU32`; nothing hands out references into it.
unsafe impl Send for SharedSems {}
unsafe impl Sync for SharedSems {}

impl Drop for SharedSems {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the mapping made in `map`.
        unsafe { libc::munmap(self.base as *mut libc::c_void, SEGMENT_BYTES); }
    }
}

/// The view of the segment while it is locked.
pub struct Locked<'a> {
    sems: &'a SharedSems,
    changed: bool,
}

impl Locked<'_> {
    fn raw(&self, i: usize) -> [u8; SLOT_BYTES] {
        let mut b = [0u8; SLOT_BYTES];
        assert!(i < MAX_SHARED_SEMS);
        // SAFETY: slot `i` lies inside the mapping (i < MAX_SHARED_SEMS);
        // the bytes are copied out, no reference is kept.
        unsafe { std::ptr::copy_nonoverlapping(self.sems.base.add(HEADER_BYTES + i * SLOT_BYTES), b.as_mut_ptr(), SLOT_BYTES); }
        b
    }

    fn put_raw(&mut self, i: usize, b: &[u8; SLOT_BYTES]) {
        assert!(i < MAX_SHARED_SEMS);
        // SAFETY: as in `raw`, copying in.
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), self.sems.base.add(HEADER_BYTES + i * SLOT_BYTES), SLOT_BYTES); }
        self.changed = true;
    }

    /// Slot `i` (`None`: free).
    pub fn slot(&self, i: usize) -> Option<Slot> { Slot::decode(&self.raw(i)) }

    /// The generation of slot `i`, free or not.
    fn slot_gen(&self, i: usize) -> u32 { get_u32(&self.raw(i), 4) }

    pub fn put(&mut self, i: usize, slot: &Slot) { self.put_raw(i, &slot.encode()); }

    /// Free slot `i`: a new generation, so its handles are dead.
    pub fn free(&mut self, i: usize) {
        let mut b = [0u8; SLOT_BYTES];
        put_u32(&mut b, 0, KIND_FREE);
        put_u32(&mut b, 4, self.slot_gen(i).wrapping_add(1) & 0x7FFF);
        self.put_raw(i, &b);
    }

    /// The semaphore behind handle `h` of kind `kind`.
    pub fn by_handle(&self, h: u32, kind: SemKind) -> Option<(usize, Slot)> {
        let i = slot_of(h)?;
        let s = self.slot(i)?;
        (handle(i, s.generation_no) == h && s.kind() == kind).then_some((i, s))
    }

    /// Sweep every semaphore (see `Slot::sweep`); those left without
    /// openers are deleted.
    pub fn sweep(&mut self) {
        for i in 0..MAX_SHARED_SEMS {
            let Some(mut s) = self.slot(i) else { continue };
            let before = s.clone();
            if !s.sweep(&*self.sems.alive) {
                self.free(i);
            } else if s != before {
                self.put(i, &s);
            }
        }
    }

    /// Mark a change the waiters must look at (without one).
    pub fn touch(&mut self) { self.changed = true; }
}

impl SharedSems {
    /// A new segment for the first process of a session.
    pub fn create(me: Proc) -> std::io::Result<SharedSems> {
        // SAFETY: a NUL-terminated literal; no other preconditions.
        let raw = unsafe { libc::memfd_create(c"warpine-semaphores".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
        if raw < 0 { return Err(std::io::Error::last_os_error()); }
        // SAFETY: a new descriptor nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let file = std::fs::File::from(fd);
        file.set_len(SEGMENT_BYTES as u64)?;
        let fd = OwnedFd::from(file);
        // SAFETY: fcntl on a descriptor this function owns.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, SEALS) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sems = Self::map(fd, me)?;
        // SAFETY: the header lies inside the mapping; nobody else has it yet.
        unsafe {
            std::ptr::write_unaligned(sems.base as *mut u32, MAGIC);
            std::ptr::write_unaligned(sems.base.add(4) as *mut u32, VERSION);
        }
        Ok(sems)
    }

    /// The segment a parent passed in `WARPINE_SEM_FD` (`Ok(None)`: none).
    pub fn from_env(me: Proc) -> std::io::Result<Option<SharedSems>> {
        let Some(text) = std::env::var_os(SEM_FD_ENV) else { return Ok(None) };
        let fd = text.to_str().and_then(|s| s.parse::<RawFd>().ok()).filter(|&n| n > 2)
            .ok_or_else(|| std::io::Error::other("malformed WARPINE_SEM_FD"))?;
        // SAFETY: fcntl on any number is safe; it fails for a closed one.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 { return Err(std::io::Error::last_os_error()); }
        // SAFETY: the descriptor is open and was handed to this process for
        // the segment alone; it is closed on exec again from here on.
        let fd = unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            OwnedFd::from_raw_fd(fd)
        };
        let sems = Self::map(fd, me)?;
        // SAFETY: the header lies inside the mapping.
        let (magic, version) = unsafe {
            (std::ptr::read_unaligned(sems.base as *const u32), std::ptr::read_unaligned(sems.base.add(4) as *const u32))
        };
        if magic != MAGIC || version != VERSION {
            return Err(std::io::Error::other("not a semaphore segment"));
        }
        Ok(Some(sems))
    }

    /// Check `fd` (a sealed memfd of the segment's size) and map it.
    fn map(fd: OwnedFd, me: Proc) -> std::io::Result<SharedSems> {
        // SAFETY: fcntl on a descriptor `fd` owns.
        let seals = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) };
        let len = std::fs::File::from(fd.try_clone()?).metadata()?.len();
        if seals < 0 || seals & SEALS != SEALS || len != SEGMENT_BYTES as u64 {
            return Err(std::io::Error::other("the semaphore segment is not a sealed memfd of its size"));
        }
        let lock_fd = std::fs::OpenOptions::new().read(true).write(true)
            .open(format!("/proc/self/fd/{}", fd.as_raw_fd()))?;
        // SAFETY: a new shared mapping of the whole (sealed, so fixed-size)
        // file; the result is checked.
        let base = unsafe {
            libc::mmap(std::ptr::null_mut(), SEGMENT_BYTES, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED, fd.as_raw_fd(), 0)
        };
        if base == libc::MAP_FAILED { return Err(std::io::Error::last_os_error()); }
        Ok(SharedSems {
            base: base as *mut u8, fd, lock_fd: OwnedFd::from(lock_fd),
            exclusive: Mutex::new(()), waiters: Mutex::new(HashMap::new()), me,
            alive: Box::new(|who: Identity| super::session_threads::proc_start_time(who.pid) == Some(who.start)),
        })
    }

    /// The same segment with another liveness test (unit tests).
    #[cfg(test)]
    fn with_alive(mut self, alive: impl Fn(Identity) -> bool + Send + Sync + 'static) -> Self {
        self.alive = Box::new(alive);
        self
    }

    pub fn me(&self) -> Proc { self.me }

    pub fn fd(&self) -> BorrowedFd<'_> { self.fd.as_fd() }

    fn generation_word(&self) -> &AtomicU32 {
        // SAFETY: the word is 4-byte aligned (the mapping is page aligned,
        // GENERATION_AT is a multiple of 4) and inside the mapping, which
        // lives as long as `self`; other processes use it atomically too.
        unsafe { &*(self.base.add(GENERATION_AT) as *const AtomicU32) }
    }

    /// The generation; read it before looking at the semaphores.
    pub fn generation(&self) -> u32 { self.generation_word().load(Ordering::SeqCst) }

    fn ofd_lock(&self, kind: libc::c_short) -> bool {
        // SAFETY: flock is plain data; fcntl on a descriptor `self` owns.
        let mut fl: libc::flock = unsafe { std::mem::zeroed() };
        fl.l_type = kind;
        fl.l_whence = libc::SEEK_SET as libc::c_short;
        fl.l_start = 0;
        fl.l_len = 1;
        loop {
            // SAFETY: as above.
            let r = unsafe { libc::fcntl(self.lock_fd.as_raw_fd(), libc::F_OFD_SETLKW, &fl) };
            if r == 0 { return true; }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted { return false; }
        }
    }

    /// Run `f` with the segment locked; a change wakes every waiter.
    pub fn with_lock<R>(&self, f: impl FnOnce(&mut Locked) -> R) -> R {
        let _g = self.exclusive.lock_or_recover();
        if !self.ofd_lock(libc::F_WRLCK as libc::c_short) {
            warn!("shared semaphores: cannot lock the segment: {}", std::io::Error::last_os_error());
        }
        let mut l = Locked { sems: self, changed: false };
        let r = f(&mut l);
        let changed = l.changed;
        self.ofd_lock(libc::F_UNLCK as libc::c_short);
        if changed {
            self.generation_word().fetch_add(1, Ordering::SeqCst);
            self.notify();
        }
        r
    }

    /// Wake every waiter, in every process, to look again.
    pub fn notify(&self) {
        // SAFETY: a futex wake on the generation word of the shared mapping
        // (non-private: other processes wait on it).
        unsafe {
            libc::syscall(libc::SYS_futex, self.generation_word().as_ptr(), libc::FUTEX_WAKE, i32::MAX, 0, 0, 0);
        }
    }

    /// Sleep until the generation is no longer `seen`, a wake, or `timeout`.
    pub fn wait_change(&self, seen: u32, timeout: Duration) {
        let ts = libc::timespec {
            tv_sec: timeout.as_secs().min(i32::MAX as u64) as libc::time_t,
            tv_nsec: timeout.subsec_nanos() as libc::c_long,
        };
        // SAFETY: futex wait on the generation word with a valid timeout;
        // it returns at once when the word is not `seen` any more.
        unsafe {
            libc::syscall(libc::SYS_futex, self.generation_word().as_ptr(), libc::FUTEX_WAIT, seen,
                &ts as *const libc::timespec, 0, 0);
        }
    }

    /// Count a thread of this process among `h`'s waiters while the guard lives.
    pub fn waiting(&self, h: u32) -> Waiting<'_> {
        *self.waiters.lock_or_recover().entry(h).or_default() += 1;
        Waiting { sems: self, h }
    }

    fn local_waiters(&self, h: u32) -> u32 {
        self.waiters.lock_or_recover().get(&h).copied().unwrap_or(0)
    }

    // ── Calls ──

    /// A new semaphore held open by this process; its handle.
    pub fn create_sem(&self, name: Option<String>, attr: u32, state: State) -> Result<u32, u32> {
        self.with_lock(|l| {
            l.sweep();
            if let Some(n) = &name && (0..MAX_SHARED_SEMS).any(|i| l.slot(i).is_some_and(|s| s.name.as_ref() == Some(n))) {
                return Err(ERROR_DUPLICATE_NAME);
            }
            let i = (0..MAX_SHARED_SEMS).find(|&i| l.slot(i).is_none()).ok_or(ERROR_TOO_MANY_SEMAPHORES)?;
            let slot = Slot { generation_no: l.slot_gen(i), attr, name, state, openers: vec![(self.me, 1)] };
            l.put(i, &slot);
            Ok(handle(i, slot.generation_no))
        })
    }

    /// Whether a semaphore of any kind has the name.
    pub fn name_exists(&self, name: &str) -> bool {
        self.with_lock(|l| (0..MAX_SHARED_SEMS).any(|i| l.slot(i).is_some_and(|s| s.name.as_deref() == Some(name))))
    }

    /// `DosOpen*Sem`: by `name` (canonical), else by handle `h`.
    pub fn open(&self, kind: SemKind, name: Option<&str>, h: u32) -> Result<u32, u32> {
        self.with_lock(|l| {
            l.sweep();
            let (i, mut s) = match name {
                Some(n) => (0..MAX_SHARED_SEMS)
                    .find_map(|i| l.slot(i).filter(|s| s.name.as_deref() == Some(n) && s.kind() == kind).map(|s| (i, s)))
                    .ok_or(ERROR_SEM_NOT_FOUND)?,
                None => l.by_handle(h, kind).ok_or(ERROR_INVALID_HANDLE)?,
            };
            let full = s.openers.len() >= MAX_OPENERS;
            match s.openers.iter_mut().find(|(p, _)| p.os2pid == self.me.os2pid) {
                Some((_, c)) if *c >= MAX_SEM_OPENS => return Err(ERROR_TOO_MANY_OPENS),
                Some((_, c)) => *c += 1,
                None if full => return Err(ERROR_TOO_MANY_OPENS),
                None => s.openers.push((self.me, 1)),
            }
            l.put(i, &s);
            Ok(handle(i, s.generation_no))
        })
    }

    /// `DosClose*Sem`: one use less for this process; the last use of the
    /// session deletes the semaphore.
    pub fn close(&self, kind: SemKind, h: u32) -> u32 {
        if self.local_waiters(h) > 0 { return ERROR_SEM_BUSY; }
        self.with_lock(|l| {
            let Some((i, mut s)) = l.by_handle(h, kind) else { return ERROR_INVALID_HANDLE };
            let Some(at) = s.openers.iter().position(|(p, _)| p.os2pid == self.me.os2pid) else {
                return ERROR_INVALID_HANDLE;
            };
            s.openers[at].1 -= 1;
            if s.openers[at].1 == 0 { s.openers.remove(at); }
            if s.sweep(&*self.alive) { l.put(i, &s) } else { l.free(i) }
            NO_ERROR
        })
    }

    /// The semaphore `h` of `kind`, opened by this process (`f` gets it and
    /// may change it; the change is written back).
    fn with_mine<R>(&self, h: u32, kind: SemKind, f: impl FnOnce(&mut Slot) -> R) -> Result<R, u32> {
        self.with_lock(|l| {
            let (i, mut s) = l.by_handle(h, kind).ok_or(ERROR_INVALID_HANDLE)?;
            if s.opens_of(self.me.os2pid) == 0 { return Err(ERROR_INVALID_HANDLE); }
            let before = s.clone();
            s.sweep(&*self.alive);
            let r = f(&mut s);
            if s != before { l.put(i, &s); }
            Ok(r)
        })
    }

    pub fn kind_of(&self, h: u32) -> Option<SemKind> {
        ALL_KINDS.into_iter().find(|&k| self.with_mine(h, k, |_| ()).is_ok())
    }

    pub fn post(&self, h: u32) -> u32 {
        self.with_mine(h, SemKind::Event, |s| s.with_event(|e| e.post()).unwrap_or(ERROR_INVALID_HANDLE))
            .unwrap_or_else(|e| e)
    }

    /// `DosResetEventSem`: the post count, or the error with count 0.
    pub fn reset(&self, h: u32) -> Result<Result<u32, u32>, u32> {
        self.with_mine(h, SemKind::Event, |s| s.with_event(|e| e.reset()).unwrap_or(Err(ERROR_INVALID_HANDLE)))
    }

    pub fn post_count(&self, h: u32) -> Result<u32, u32> {
        self.with_mine(h, SemKind::Event, |s| match s.state { State::Event { post_count } => post_count, _ => 0 })
    }

    /// One look for a waiter on event `h`: true (and the post consumed as
    /// the flags say) when it is posted.
    pub fn try_event(&self, h: u32) -> Result<bool, u32> {
        self.with_mine(h, SemKind::Event, |s| {
            s.with_event(|e| { let p = e.posted(); if p { e.consume(); } p }).unwrap_or(false)
        })
    }

    /// One look for thread `tid` requesting mutex `h`: `Some(died)` when it
    /// took it.
    pub fn try_mutex(&self, h: u32, tid: u32) -> Result<Option<bool>, u32> {
        let who = Owner { proc: self.me, tid };
        self.with_mine(h, SemKind::Mutex, |s| s.mutex_available(who).then(|| s.mutex_acquire(who)))
    }

    pub fn release(&self, h: u32, tid: u32) -> u32 {
        let who = Owner { proc: self.me, tid };
        self.with_mine(h, SemKind::Mutex, |s| match &mut s.state {
            State::Mutex { owner, count, died } if died.is_none() && *owner == Some(who) => {
                *count -= 1;
                if *count == 0 { *owner = None; }
                NO_ERROR
            }
            _ => ERROR_NOT_OWNER,
        }).unwrap_or_else(|e| e)
    }

    /// `DosQueryMutexSem`: owner process, thread, count and the result.
    pub fn query_mutex(&self, h: u32) -> Result<(u32, u32, u32, u32), u32> {
        self.with_mine(h, SemKind::Mutex, |s| match &s.state {
            State::Mutex { died: Some((pid, tid)), .. } => (*pid, *tid, 0, ERROR_SEM_OWNER_DIED),
            State::Mutex { owner: Some(o), count, .. } => (o.proc.os2pid, o.tid, *count, NO_ERROR),
            _ => (0, 0, 0, NO_ERROR),
        })
    }

    /// The flags and name of `h` (any kind this process has open).
    pub fn attr(&self, h: u32) -> Option<u32> {
        ALL_KINDS.into_iter().find_map(|k| self.with_mine(h, k, |s| s.attr).ok())
    }

    /// Thread `tid` of this process ended: its mutexes are owner-died.
    pub fn owner_ended(&self, tid: u32) {
        let me = self.me;
        self.with_lock(|l| {
            for i in 0..MAX_SHARED_SEMS {
                let Some(mut s) = l.slot(i) else { continue };
                if let State::Mutex { owner, count, died } = &mut s.state
                    && owner.is_some_and(|o| o.proc.os2pid == me.os2pid && o.tid == tid) {
                    *died = Some((me.os2pid, tid));
                    *owner = None;
                    *count = 0;
                    l.put(i, &s);
                }
            }
        });
    }

    /// This process ends: it closes every shared semaphore it has open,
    /// and the mutexes it owns are owner-died.
    pub fn leave(&self) {
        let me = self.me;
        self.with_lock(|l| {
            for i in 0..MAX_SHARED_SEMS {
                let Some(mut s) = l.slot(i) else { continue };
                let before = s.clone();
                s.openers.retain(|(p, _)| p.os2pid != me.os2pid);
                if let State::Mutex { owner, count, died } = &mut s.state
                    && let Some(o) = *owner && o.proc.os2pid == me.os2pid {
                    *died = Some((me.os2pid, o.tid));
                    *owner = None;
                    *count = 0;
                }
                if s.openers.is_empty() { l.free(i) } else if s != before { l.put(i, &s) }
            }
        });
    }

    // ── Shared muxwait lists ──

    /// The kind `records` join a list of `existing` ones as: each a shared
    /// event or mutex this process has open (the caller turns away private
    /// handles first).
    fn check_records(&self, l: &Locked, list: Option<SemKind>, existing: &[MuxWaitRecord],
                     records: &[MuxWaitRecord]) -> Result<Option<SemKind>, u32> {
        let mine = |h: u32| ALL_KINDS.into_iter().find(|&k| {
            l.by_handle(h, k).is_some_and(|(_, s)| s.opens_of(self.me.os2pid) > 0)
        });
        check_mux_records(list, existing, records, mine)
    }

    /// `DosCreateMuxWaitSem` of a shared list: its handle.
    pub fn create_mux(&self, name: Option<String>, attr: u32, records: &[MuxWaitRecord]) -> Result<u32, u32> {
        check_mux_attr(attr)?;
        let list = self.with_lock(|l| self.check_records(l, None, &[], records))?;
        self.create_sem(name, attr, State::MuxWait { records: records.to_vec(), list })
    }

    /// `DosAddMuxWaitSem` to a shared list.
    pub fn add_mux_record(&self, h: u32, rec: MuxWaitRecord) -> u32 {
        self.with_lock(|l| {
            let Some((i, mut s)) = l.by_handle(h, SemKind::MuxWait) else { return ERROR_INVALID_HANDLE };
            if s.opens_of(self.me.os2pid) == 0 { return ERROR_INVALID_HANDLE; }
            let State::MuxWait { records, list } = &mut s.state else { return ERROR_INVALID_HANDLE };
            match self.check_records(l, *list, records, &[rec]) {
                Ok(k) => {
                    *list = k;
                    records.push(rec);
                    l.put(i, &s);
                    NO_ERROR
                }
                Err(e) => e,
            }
        })
    }

    /// `DosDeleteMuxWaitSem` from a shared list.
    pub fn delete_mux_record(&self, h: u32, hsem: u32) -> u32 {
        self.with_mine(h, SemKind::MuxWait, |s| {
            let State::MuxWait { records, list } = &mut s.state else { return ERROR_INVALID_HANDLE };
            if records.is_empty() { return ERROR_EMPTY_MUXWAIT; }
            let Some(at) = records.iter().position(|r| r.hsem == hsem) else { return ERROR_INVALID_HANDLE };
            records.remove(at);
            if records.is_empty() { *list = None; }
            NO_ERROR
        }).unwrap_or_else(|e| e)
    }

    /// `DosQueryMuxWaitSem` of a shared list: the records and the flags.
    pub fn query_mux(&self, h: u32) -> Result<(Vec<MuxWaitRecord>, u32), u32> {
        self.with_mine(h, SemKind::MuxWait, |s| match &s.state {
            State::MuxWait { records, .. } => (records.clone(), s.attr | DC_SEM_SHARED),
            _ => (Vec::new(), s.attr),
        })
    }

    /// One look for thread `tid` waiting on shared list `h`, as
    /// `SemaphoreManager::try_mux`: the user value of the record that ends
    /// the wait and whether a mutex it took had lost its owner, or `None`.
    /// A member nobody has open any more is `ERROR_INVALID_HANDLE`.
    pub fn try_mux(&self, h: u32, tid: u32) -> Result<Option<(u32, bool)>, u32> {
        let who = Owner { proc: self.me, tid };
        self.with_lock(|l| {
            let (_, mux) = l.by_handle(h, SemKind::MuxWait).ok_or(ERROR_INVALID_HANDLE)?;
            if mux.opens_of(self.me.os2pid) == 0 { return Err(ERROR_INVALID_HANDLE); }
            let State::MuxWait { records, list } = &mux.state else { return Err(ERROR_INVALID_HANDLE) };
            let (Some(last), Some(want)) = (records.last(), *list) else { return Err(ERROR_EMPTY_MUXWAIT) };
            let wait_all = mux.attr & DCMW_WAIT_ALL != 0;
            let mut members = Vec::with_capacity(records.len());
            for r in records {
                let (i, s) = l.by_handle(r.hsem, want).ok_or(ERROR_INVALID_HANDLE)?;
                let mut swept = s.clone();
                swept.sweep(&*self.alive);
                members.push((i, s, swept));
            }
            let ready = |s: &Slot| match &s.state {
                State::Event { post_count } => *post_count > 0,
                _ => s.mutex_available(who),
            };
            let take = |s: &mut Slot| match s.state {
                State::Event { .. } => { s.with_event(|e| e.consume()); false }
                _ => s.mutex_acquire(who),
            };
            let chosen: Vec<usize> = if wait_all {
                if !members.iter().all(|(_, _, s)| ready(s)) { return Ok(None); }
                (0..members.len()).collect()
            } else {
                match members.iter().position(|(_, _, s)| ready(s)) {
                    Some(i) => vec![i],
                    None => return Ok(None),
                }
            };
            let died = chosen.iter().fold(false, |d, &k| take(&mut members[k].2) | d);
            for (i, before, after) in &members {
                if after != before { l.put(*i, after); }
            }
            let user = if wait_all { last.user } else { records[chosen[0]].user };
            Ok(Some((user, died)))
        })
    }

    /// Run `f` on several semaphores at once, all under the lock (muxwait
    /// lists): `f` gets a reader/changer for this process's semaphores.
    pub fn with_many<R>(&self, f: impl FnOnce(&mut Many) -> R) -> R {
        self.with_lock(|l| {
            let mut m = Many { l, me: self.me, alive: &*self.alive };
            f(&mut m)
        })
    }
}

/// Several semaphores looked at under one lock (`SharedSems::with_many`).
pub struct Many<'l, 'a> {
    l: &'l mut Locked<'a>,
    me: Proc,
    alive: &'l dyn Fn(Identity) -> bool,
}

impl Many<'_, '_> {
    fn get(&self, h: u32, kind: SemKind) -> Option<(usize, Slot)> {
        let (i, mut s) = self.l.by_handle(h, kind)?;
        if s.opens_of(self.me.os2pid) == 0 { return None; }
        s.sweep(self.alive);
        Some((i, s))
    }

    pub fn kind_of(&self, h: u32) -> Option<SemKind> {
        ALL_KINDS.into_iter().find(|&k| self.get(h, k).is_some())
    }

    pub fn event_posted(&self, h: u32) -> Option<bool> {
        let (_, s) = self.get(h, SemKind::Event)?;
        Some(matches!(s.state, State::Event { post_count } if post_count > 0))
    }

    pub fn event_consume(&mut self, h: u32) {
        if let Some((i, mut s)) = self.get(h, SemKind::Event) {
            s.with_event(|e| e.consume());
            self.l.put(i, &s);
        }
    }

    pub fn mutex_available(&self, h: u32, tid: u32) -> Option<bool> {
        let (_, s) = self.get(h, SemKind::Mutex)?;
        Some(s.mutex_available(Owner { proc: self.me, tid }))
    }

    /// Take mutex `h` for `tid`; true when its owner had died.
    pub fn mutex_acquire(&mut self, h: u32, tid: u32) -> bool {
        let Some((i, mut s)) = self.get(h, SemKind::Mutex) else { return false };
        let died = s.mutex_acquire(Owner { proc: self.me, tid });
        self.l.put(i, &s);
        died
    }
}

/// A thread of this process counted among a shared semaphore's waiters.
pub struct Waiting<'a> {
    sems: &'a SharedSems,
    h: u32,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut w = self.sems.waiters.lock_or_recover();
        if let Some(n) = w.get_mut(&self.h) {
            *n = n.saturating_sub(1);
            if *n == 0 { w.remove(&self.h); }
        }
    }
}

impl super::SharedState {
    /// Attach the session's semaphore segment: the one a parent passed, or a
    /// new one for the first process. Without a session (or when anything
    /// fails) shared semaphores stay within this process.
    pub fn attach_shared_sems(&self) {
        let me = {
            let slots = self.thread_slots.lock_or_recover();
            let Some(s) = slots.session() else { return };
            Proc { os2pid: s.os2pid(), host: s.identity() }
        };
        let sems = match SharedSems::from_env(me) {
            Ok(Some(s)) => s,
            Ok(None) => match SharedSems::create(me) {
                Ok(s) => s,
                Err(e) => { warn!("shared semaphores: cannot make the segment: {e}"); return; }
            },
            Err(e) => {
                warn!("shared semaphores: the segment passed on is unusable ({e}); semaphores stay in this process");
                return;
            }
        };
        let sems = std::sync::Arc::new(sems);
        if self.shared_sems.set(sems.clone()).is_ok() {
            self.sem_mgr.lock_or_recover().shared = Some(sems);
        }
    }
}

/// Pass the segment to the child `cmd` starts (close-on-exec cleared in
/// the forked child only).
pub fn pass_to(sems: &SharedSems, cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let fd = sems.fd.as_raw_fd();
    cmd.env(SEM_FD_ENV, fd.to_string());
    // SAFETY: the hook runs in the forked child before `exec` and only calls
    // fcntl (async-signal-safe) on a descriptor the segment keeps open.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(os2pid: u32, pid: u32) -> Proc { Proc { os2pid, host: Identity { pid, start: 7 } } }

    /// Two processes (1, host 100; 2, host 200) on one segment; host 300 is
    /// dead.
    fn pair() -> (SharedSems, SharedSems) {
        let alive = |who: Identity| who.pid != 300;
        let a = SharedSems::create(proc(1, 100)).unwrap().with_alive(alive);
        let fd = a.fd.try_clone().unwrap();
        let b = SharedSems::map(fd, proc(2, 200)).unwrap().with_alive(alive);
        (a, b)
    }

    #[test]
    fn slots_round_trip_and_reject_junk() {
        let s = Slot {
            generation_no: 3, attr: DC_SEM_SHARED, name: Some("\\SEM32\\X".into()),
            state: State::Mutex { owner: Some(Owner { proc: proc(1, 100), tid: 2 }), count: 2, died: None },
            openers: vec![(proc(1, 100), 1), (proc(2, 200), 4)],
        };
        assert_eq!(Slot::decode(&s.encode()), Some(s.clone()));
        let e = Slot { generation_no: 0, attr: 0, name: None, state: State::Event { post_count: 5 }, openers: vec![(proc(1, 1), 1)] };
        assert_eq!(Slot::decode(&e.encode()), Some(e.clone()));
        assert_eq!(Slot::decode(&[0u8; SLOT_BYTES]), None, "free");
        let mut b = e.encode();
        put_u32(&mut b, 0, 9);
        assert_eq!(Slot::decode(&b), None, "unknown kind");
        let mut b = e.encode();
        put_u32(&mut b, 52, 0);
        assert_eq!(Slot::decode(&b), None, "no openers");
        let mut b = e.encode();
        put_u32(&mut b, 52, MAX_OPENERS as u32 + 1);
        assert_eq!(Slot::decode(&b), None);
        let mut b = e.encode();
        put_u32(&mut b, 48, 3);
        b[NAME_AT..NAME_AT + 3].copy_from_slice(b"abc");
        assert_eq!(Slot::decode(&b), None, "not a \\SEM32\\ name");
        let mut b = e.encode();
        put_u32(&mut b, 12, MAX_EVENT_POSTS + 1);
        assert_eq!(Slot::decode(&b), None);
        let mut b = s.encode();
        put_u32(&mut b, 28, 0);
        assert_eq!(Slot::decode(&b), None, "an owner with no count");
        let mut b = s.encode();
        put_u32(&mut b, 40, 1);
        assert_eq!(Slot::decode(&b), None, "owned and died");
    }

    #[test]
    fn handles_name_the_slot_and_its_generation() {
        let h = handle(5, 9);
        assert!(is_shared(h));
        assert_eq!(slot_of(h), Some(5));
        assert_eq!(slot_of(handle(MAX_SHARED_SEMS, 0)), None);
        assert!(!is_shared(7));
    }

    #[test]
    fn processes_share_named_events() {
        let (a, b) = pair();
        let h = a.create_sem(Some("\\SEM32\\EV".into()), 0, State::Event { post_count: 0 }).unwrap();
        assert_eq!(b.create_sem(Some("\\SEM32\\EV".into()), 0, State::Event { post_count: 0 }), Err(ERROR_DUPLICATE_NAME));
        assert_eq!(b.post(h), ERROR_INVALID_HANDLE, "not opened by b yet");
        assert_eq!(b.open(SemKind::Event, Some("\\SEM32\\EV"), 0), Ok(h));
        assert_eq!(b.open(SemKind::Mutex, Some("\\SEM32\\EV"), 0), Err(ERROR_SEM_NOT_FOUND));
        let seen = a.generation();
        assert_eq!(b.post(h), NO_ERROR);
        assert_ne!(a.generation(), seen, "a post wakes waiters");
        assert_eq!(b.post(h), ERROR_ALREADY_POSTED);
        assert_eq!(a.try_event(h), Ok(true));
        assert_eq!(a.post_count(h), Ok(2), "a plain event stays posted");
        assert_eq!(a.reset(h), Ok(Ok(2)));
        assert_eq!(a.reset(h), Ok(Err(ERROR_ALREADY_RESET)));
        assert_eq!(a.close(SemKind::Event, h), NO_ERROR);
        assert_eq!(a.post(h), ERROR_INVALID_HANDLE);
        assert_eq!(b.close(SemKind::Event, h), NO_ERROR);
        assert_eq!(b.open(SemKind::Event, None, h), Err(ERROR_INVALID_HANDLE), "deleted with the last close");
        let h2 = a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 1 }).unwrap();
        assert_ne!(h2, h, "a reused slot has a new generation");
        assert_eq!(b.open(SemKind::Event, None, h2), Ok(h2), "opened by handle");
    }

    #[test]
    fn mutexes_pass_between_processes_and_die_with_their_owner() {
        let (a, b) = pair();
        let h = a.create_sem(Some("\\SEM32\\MX".into()), 0, State::Mutex {
            owner: Some(Owner { proc: a.me(), tid: 1 }), count: 1, died: None }).unwrap();
        b.open(SemKind::Mutex, None, h).unwrap();
        assert_eq!(b.try_mutex(h, 1), Ok(None), "owned by a");
        assert_eq!(b.release(h, 1), ERROR_NOT_OWNER);
        assert_eq!(a.try_mutex(h, 1), Ok(Some(false)), "recursive");
        assert_eq!(b.query_mutex(h), Ok((1, 1, 2, NO_ERROR)));
        assert_eq!(a.release(h, 1), NO_ERROR);
        assert_eq!(a.release(h, 1), NO_ERROR);
        assert_eq!(b.try_mutex(h, 3), Ok(Some(false)));
        // b's thread 3 ends owning it.
        b.owner_ended(3);
        assert_eq!(a.query_mutex(h), Ok((2, 3, 0, ERROR_SEM_OWNER_DIED)));
        assert_eq!(a.try_mutex(h, 1), Ok(Some(true)), "the next requester gets it, owner died");
        // a process ending while it owns one.
        a.leave();
        assert_eq!(b.query_mutex(h), Ok((1, 1, 0, ERROR_SEM_OWNER_DIED)));
        assert_eq!(a.query_mutex(h), Err(ERROR_INVALID_HANDLE), "a closed it by ending");
    }

    #[test]
    fn dead_processes_are_swept() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let ghost_alive = Arc::new(AtomicBool::new(true));
        let alive = { let g = ghost_alive.clone(); move |who: Identity| who.pid != 300 || g.load(Ordering::SeqCst) } ;
        let a = SharedSems::create(proc(1, 100)).unwrap().with_alive(alive.clone());
        let b = SharedSems::map(a.fd.try_clone().unwrap(), proc(2, 200)).unwrap().with_alive(alive.clone());
        let ghost = SharedSems::map(a.fd.try_clone().unwrap(), proc(3, 300)).unwrap().with_alive(alive);
        // Process 3 owns a mutex b has open too, and has an event alone.
        let h = ghost.create_sem(None, DC_SEM_SHARED, State::Mutex {
            owner: Some(Owner { proc: ghost.me(), tid: 4 }), count: 1, died: None }).unwrap();
        b.open(SemKind::Mutex, None, h).unwrap();
        ghost.create_sem(Some("\\SEM32\\LONE".into()), 0, State::Event { post_count: 0 }).unwrap();
        assert_eq!(b.query_mutex(h), Ok((3, 4, 1, NO_ERROR)));
        // It is killed: nothing of its own runs any more.
        ghost_alive.store(false, Ordering::SeqCst);
        assert_eq!(b.query_mutex(h), Ok((3, 4, 0, ERROR_SEM_OWNER_DIED)), "its owner is gone");
        assert_eq!(b.try_mutex(h, 1), Ok(Some(true)));
        assert_eq!(a.open(SemKind::Event, Some("\\SEM32\\LONE"), 0), Err(ERROR_SEM_NOT_FOUND),
                   "a semaphore only it had open is gone");
    }

    #[test]
    fn closing_while_a_thread_waits_is_busy_and_opens_are_bounded() {
        let (a, b) = pair();
        let h = a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 0 }).unwrap();
        {
            let _w = a.waiting(h);
            assert_eq!(a.close(SemKind::Event, h), ERROR_SEM_BUSY);
        }
        assert_eq!(b.close(SemKind::Event, h), ERROR_INVALID_HANDLE, "b never opened it");
        assert_eq!(a.close(SemKind::Event, h), NO_ERROR);
        // The segment fills up.
        let mut n = 0;
        while a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 0 }).is_ok() { n += 1; }
        assert_eq!(n, MAX_SHARED_SEMS);
        assert_eq!(a.create_sem(None, 0, State::Event { post_count: 0 }), Err(ERROR_TOO_MANY_SEMAPHORES));
    }

    #[test]
    fn waiters_in_another_process_are_woken() {
        let (a, b) = pair();
        let h = a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 0 }).unwrap();
        b.open(SemKind::Event, None, h).unwrap();
        let b = std::sync::Arc::new(b);
        let waiter = {
            let b = b.clone();
            std::thread::spawn(move || {
                let start = std::time::Instant::now();
                loop {
                    let seen = b.generation();
                    if b.try_event(h).unwrap() { return start.elapsed(); }
                    b.wait_change(seen, Duration::from_secs(5));
                }
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(a.post(h), NO_ERROR);
        assert!(waiter.join().unwrap() < Duration::from_secs(4), "woken by the post, not the timeout");
    }

    /// A mock loader whose shared semaphores are a segment of its own.
    fn loader_with_sems() -> super::super::Loader {
        let l = super::super::Loader::new_mock();
        let pid = std::process::id();
        let me = Proc {
            os2pid: l.shared.process_id(),
            host: Identity { pid, start: super::super::session_threads::proc_start_time(pid).unwrap() },
        };
        let sems = std::sync::Arc::new(SharedSems::create(me).unwrap());
        let _ = l.shared.shared_sems.set(sems.clone());
        l.shared.sem_mgr.lock_or_recover().shared = Some(sems);
        l
    }

    #[test]
    fn the_api_calls_route_shared_semaphores() {
        let l = loader_with_sems();
        let ph = DYNAMIC_ALLOC_BASE;
        assert_eq!(l.dos_create_event_sem(0, ph, DC_SEM_SHARED, 0), NO_ERROR);
        let shared = l.guest_read::<u32>(ph).unwrap();
        assert!(is_shared(shared), "a DC_SEM_SHARED event is the session's");
        assert_eq!(l.dos_create_event_sem(0, ph + 4, 0, 0), NO_ERROR);
        let private = l.guest_read::<u32>(ph + 4).unwrap();
        assert!(!is_shared(private), "a private event stays in the process");
        assert!(l.shared.sem_mgr.lock_or_recover().is_shared_event(shared), "timers can post it");
        // A muxwait list of both: any, then all.
        for (i, h) in [shared, private].into_iter().enumerate() {
            l.guest_write::<u32>(ph + 16 + 8 * i as u32, h);
            l.guest_write::<u32>(ph + 20 + 8 * i as u32, 10 + i as u32);
        }
        assert_eq!(l.dos_create_mux_wait_sem(0, ph + 8, 2, ph + 16, DCMW_WAIT_ANY), NO_ERROR);
        let any = l.guest_read::<u32>(ph + 8).unwrap();
        assert_eq!(l.dos_create_mux_wait_sem(0, ph + 12, 2, ph + 16, DCMW_WAIT_ALL), NO_ERROR);
        let all = l.guest_read::<u32>(ph + 12).unwrap();
        assert_eq!(l.dos_wait_mux_wait_sem(1, any, 0, ph + 40), ERROR_TIMEOUT);
        assert_eq!(l.dos_post_event_sem(shared), NO_ERROR);
        assert_eq!(l.dos_wait_mux_wait_sem(1, any, 0, ph + 40), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ph + 40), Some(10), "the shared member");
        assert_eq!(l.dos_wait_mux_wait_sem(1, all, 0, ph + 40), ERROR_TIMEOUT);
        assert_eq!(l.dos_post_event_sem(private), NO_ERROR);
        assert_eq!(l.dos_wait_mux_wait_sem(1, all, 0, ph + 40), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ph + 40), Some(11), "the last record");
        // Named mutexes are shared; a name is unique across both tables.
        let name = ph + 64;
        l.guest_write_bytes(name, b"\\SEM32\\M\0");
        assert_eq!(l.dos_create_mutex_sem(1, name, ph + 48, 0, 1), NO_ERROR);
        let m = l.guest_read::<u32>(ph + 48).unwrap();
        assert!(is_shared(m));
        assert_eq!(l.dos_create_mux_wait_sem(name, ph + 52, 0, ph + 16, DCMW_WAIT_ANY), ERROR_DUPLICATE_NAME);
        assert_eq!(l.dos_request_mutex_sem(1, m, 0), NO_ERROR, "recursive");
        assert_eq!(l.dos_release_mutex_sem(1, m), NO_ERROR);
        assert_eq!(l.dos_release_mutex_sem(1, m), NO_ERROR);
        assert_eq!(l.dos_release_mutex_sem(1, m), ERROR_NOT_OWNER);
        assert_eq!(l.dos_close_mutex_sem(m), NO_ERROR);
        assert_eq!(l.dos_close_mutex_sem(m), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn muxwait_slots_round_trip_and_reject_junk() {
        let rec = |hsem, user| MuxWaitRecord { hsem, user };
        let m = Slot {
            generation_no: 1, attr: DC_SEM_SHARED | DCMW_WAIT_ALL, name: Some("\\SEM32\\MUX".into()),
            state: State::MuxWait { records: vec![rec(handle(3, 0), 7), rec(handle(4, 2), 8)], list: Some(SemKind::Mutex) },
            openers: vec![(proc(1, 100), 1)],
        };
        assert_eq!(Slot::decode(&m.encode()), Some(m.clone()));
        let empty = Slot { state: State::MuxWait { records: vec![], list: None }, attr: DCMW_WAIT_ANY, ..m.clone() };
        assert_eq!(Slot::decode(&empty.encode()), Some(empty.clone()));
        let mut b = m.encode();
        put_u32(&mut b, 12, MAX_MUXWAIT_SEMS as u32 + 1);
        assert_eq!(Slot::decode(&b), None, "too many records");
        let mut b = m.encode();
        put_u32(&mut b, 16, 0);
        assert_eq!(Slot::decode(&b), None, "records without a kind");
        let mut b = m.encode();
        put_u32(&mut b, 16, KIND_MUXWAIT);
        assert_eq!(Slot::decode(&b), None, "a muxwait list in a list");
        let mut b = m.encode();
        put_u32(&mut b, RECORDS_AT, 5);
        assert_eq!(Slot::decode(&b), None, "a private handle");
        let mut b = m.encode();
        put_u32(&mut b, RECORDS_AT + RECORD_BYTES, handle(3, 0));
        assert_eq!(Slot::decode(&b), None, "a handle twice");
        let mut b = m.encode();
        put_u32(&mut b, 8, DCMW_WAIT_ALL | DCMW_WAIT_ANY);
        assert_eq!(Slot::decode(&b), None, "both waits");
    }

    #[test]
    fn processes_share_muxwait_lists() {
        let (a, b) = pair();
        let rec = |hsem, user| MuxWaitRecord { hsem, user };
        let e1 = a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 0 }).unwrap();
        let e2 = a.create_sem(None, DC_SEM_SHARED, State::Event { post_count: 0 }).unwrap();
        let mux = a.create_mux(Some("\\SEM32\\ANY".into()), DCMW_WAIT_ANY, &[rec(e1, 1), rec(e2, 2)]).unwrap();
        assert_eq!(a.kind_of(mux), Some(SemKind::MuxWait));
        assert_eq!(a.create_mux(None, DCMW_WAIT_ANY, &[rec(mux, 1)]), Err(ERROR_WRONG_TYPE), "no muxwait in a list");
        assert_eq!(a.create_mux(None, DCMW_WAIT_ANY, &[rec(e1, 1), rec(e1, 2)]), Err(ERROR_DUPLICATE_HANDLE));
        assert_eq!(b.create_mux(None, DCMW_WAIT_ANY, &[rec(e1, 1)]), Err(ERROR_INVALID_HANDLE), "b has not opened e1");
        assert_eq!(a.create_mux(None, 0, &[rec(e1, 1)]), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(b.try_mux(mux, 1), Err(ERROR_INVALID_HANDLE), "not opened by b yet");
        assert_eq!(b.open(SemKind::MuxWait, Some("\\SEM32\\ANY"), 0), Ok(mux));
        assert_eq!(b.open(SemKind::Event, Some("\\SEM32\\ANY"), 0), Err(ERROR_SEM_NOT_FOUND));
        // b waits on members only a has open.
        assert_eq!(b.try_mux(mux, 1), Ok(None));
        let seen = b.generation();
        assert_eq!(a.post(e2), NO_ERROR);
        assert_ne!(b.generation(), seen);
        assert_eq!(b.try_mux(mux, 1), Ok(Some((2, false))));
        assert_eq!(b.query_mux(mux), Ok((vec![rec(e1, 1), rec(e2, 2)], DCMW_WAIT_ANY | DC_SEM_SHARED)));
        // The list changes for every process.
        assert_eq!(a.delete_mux_record(mux, e2), NO_ERROR);
        assert_eq!(a.delete_mux_record(mux, e2), ERROR_INVALID_HANDLE);
        assert_eq!(b.try_mux(mux, 1), Ok(None), "e2 left the list");
        assert_eq!(a.delete_mux_record(mux, e1), NO_ERROR);
        assert_eq!(b.try_mux(mux, 1), Err(ERROR_EMPTY_MUXWAIT));
        assert_eq!(a.delete_mux_record(mux, e1), ERROR_EMPTY_MUXWAIT);
        assert_eq!(a.add_mux_record(mux, rec(e1, 5)), NO_ERROR);
        assert_eq!(a.add_mux_record(mux, rec(e1, 5)), ERROR_DUPLICATE_HANDLE);
        assert_eq!(b.add_mux_record(mux, rec(e2, 6)), ERROR_INVALID_HANDLE, "b has not opened e2");
        // A member closed by its last opener ends the wait.
        assert_eq!(a.close(SemKind::Event, e1), NO_ERROR);
        assert_eq!(b.try_mux(mux, 1), Err(ERROR_INVALID_HANDLE));
        assert_eq!(a.close(SemKind::MuxWait, mux), NO_ERROR);
        assert_eq!(b.close(SemKind::MuxWait, mux), NO_ERROR);
        assert_eq!(b.open(SemKind::MuxWait, None, mux), Err(ERROR_INVALID_HANDLE), "deleted with the last close");
    }

    #[test]
    fn a_shared_muxwait_takes_every_mutex_and_sees_dead_owners() {
        let (a, b) = pair();
        let rec = |hsem, user| MuxWaitRecord { hsem, user };
        let held = State::Mutex { owner: Some(Owner { proc: proc(1, 100), tid: 1 }), count: 1, died: None };
        let m1 = a.create_sem(None, DC_SEM_SHARED, held).unwrap();
        let m2 = a.create_sem(None, DC_SEM_SHARED, State::Mutex { owner: None, count: 0, died: None }).unwrap();
        let mux = a.create_mux(None, DC_SEM_SHARED | DCMW_WAIT_ALL, &[rec(m1, 1), rec(m2, 2)]).unwrap();
        assert_eq!(b.open(SemKind::MuxWait, None, mux), Ok(mux));
        assert_eq!(b.try_mux(mux, 4), Ok(None), "a owns m1");
        assert_eq!(a.try_mux(mux, 1), Ok(Some((2, false))), "the owner takes both");
        assert_eq!(a.query_mutex(m1), Ok((1, 1, 2, NO_ERROR)));
        assert_eq!(a.query_mutex(m2), Ok((1, 1, 1, NO_ERROR)));
        assert_eq!(b.try_mux(mux, 4), Ok(None));
        assert_eq!(a.release(m2, 1), NO_ERROR);
        assert_eq!(b.try_mux(mux, 4), Ok(None), "all of them, not just m2");
        a.owner_ended(1);
        assert_eq!(b.try_mux(mux, 4), Ok(Some((2, true))), "m1's owner died");
        assert_eq!(a.query_mutex(m1), Ok((2, 4, 1, NO_ERROR)));
        assert_eq!(a.query_mutex(m2), Ok((2, 4, 1, NO_ERROR)));
    }

    #[test]
    fn the_api_calls_route_shared_muxwait_lists() {
        let l = loader_with_sems();
        let ph = DYNAMIC_ALLOC_BASE;
        assert_eq!(l.dos_create_event_sem(0, ph, DC_SEM_SHARED, 0), NO_ERROR);
        let shared = l.guest_read::<u32>(ph).unwrap();
        assert_eq!(l.dos_create_event_sem(0, ph + 4, 0, 0), NO_ERROR);
        let private = l.guest_read::<u32>(ph + 4).unwrap();
        l.guest_write::<u32>(ph + 16, shared);
        l.guest_write::<u32>(ph + 20, 10);
        l.guest_write::<u32>(ph + 24, private);
        l.guest_write::<u32>(ph + 28, 11);
        let name = ph + 64;
        l.guest_write_bytes(name, b"\\SEM32\\LIST\0");
        assert_eq!(l.dos_create_mux_wait_sem(name, ph + 8, 2, ph + 16, DCMW_WAIT_ANY), ERROR_WRONG_TYPE,
                   "a shared list holds shared semaphores only");
        l.guest_write::<u32>(ph + 24, 0x1234);
        assert_eq!(l.dos_create_mux_wait_sem(name, ph + 8, 2, ph + 16, DCMW_WAIT_ANY), ERROR_INVALID_HANDLE);
        assert_eq!(l.dos_create_mux_wait_sem(name, ph + 8, 1, ph + 16, DCMW_WAIT_ANY), NO_ERROR);
        let mux = l.guest_read::<u32>(ph + 8).unwrap();
        assert!(is_shared(mux), "a named muxwait list is the session's");
        assert_eq!(l.dos_create_mux_wait_sem(name, ph + 12, 1, ph + 16, DCMW_WAIT_ANY), ERROR_DUPLICATE_NAME);
        assert_eq!(l.dos_wait_mux_wait_sem(1, mux, 0, ph + 40), ERROR_TIMEOUT);
        assert_eq!(l.dos_post_event_sem(shared), NO_ERROR);
        assert_eq!(l.dos_wait_mux_wait_sem(1, mux, 0, ph + 40), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ph + 40), Some(10));
        l.guest_write::<u32>(ph + 24, private);
        assert_eq!(l.dos_add_mux_wait_sem(mux, ph + 24), ERROR_WRONG_TYPE);
        l.guest_write::<u32>(ph + 48, 4);
        assert_eq!(l.dos_query_mux_wait_sem(mux, ph + 48, ph + 80, ph + 52), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ph + 48), Some(1));
        assert_eq!(l.guest_read::<u32>(ph + 80), Some(shared));
        assert_eq!(l.guest_read::<u32>(ph + 52), Some(DCMW_WAIT_ANY | DC_SEM_SHARED));
        l.guest_write::<u32>(ph + 12, 0);
        assert_eq!(l.dos_open_mux_wait_sem(name, ph + 12), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(ph + 12), Some(mux));
        assert_eq!(l.dos_delete_mux_wait_sem(mux, shared), NO_ERROR);
        assert_eq!(l.dos_wait_mux_wait_sem(1, mux, 0, ph + 40), ERROR_EMPTY_MUXWAIT);
        assert_eq!(l.dos_close_mux_wait_sem(mux), NO_ERROR);
        assert_eq!(l.dos_close_mux_wait_sem(mux), NO_ERROR);
        assert_eq!(l.dos_close_mux_wait_sem(mux), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn a_segment_is_checked_before_use() {
        let plain = OwnedFd::from(tempfile::tempfile().unwrap());
        assert!(SharedSems::map(plain, proc(1, 1)).is_err(), "not a sealed memfd");
    }
}
