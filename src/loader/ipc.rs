// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Semaphores (event, mutex, muxwait) and queues of the OS/2 process.
//!
//! Semaphore behaviour from IBM's *Control Program Programming Reference*
//! and `<bsedos.h>` / `<bseerr.h>`:
//! - an event semaphore counts its posts: `DosPostEventSem` adds one
//!   (`ERROR_ALREADY_POSTED` when it was already posted, the count still
//!   goes up; `ERROR_TOO_MANY_POSTS` past 65535), `DosResetEventSem`
//!   returns the count and clears it (`ERROR_ALREADY_RESET` at 0),
//!   `DosQueryEventSem` returns it;
//! - a mutex semaphore has an owner thread and a request count (requests
//!   less releases), 0 when unowned (`DosQueryMutexSem`);
//! - a muxwait list holds at most 64 semaphores, all events or all mutexes
//!   (`ERROR_WRONG_TYPE`; a muxwait semaphore cannot be in a list), each
//!   once (`ERROR_DUPLICATE_HANDLE`); `DosWaitMuxWaitSem` waits for any or
//!   all of them (`DCMW_WAIT_ANY` / `DCMW_WAIT_ALL`) and takes the mutexes
//!   it waited for;
//! - named semaphores start with `\SEM32\` (`ERROR_INVALID_NAME`), are
//!   unique (`ERROR_DUPLICATE_NAME`) and shared; opening one by name or
//!   handle adds to its usage count, closing takes from it and the last
//!   close deletes the semaphore;
//! - closing a semaphore another thread is blocked on fails with
//!   `ERROR_SEM_BUSY` (EDM2: "Another thread in this process is blocked on
//!   the semaphore"); an owned mutex nobody waits for closes (the
//!   reference does not call it busy);
//! - a mutex whose owner thread ended without releasing it is left in the
//!   "owner died" state: `DosQueryMutexSem` reports the dead owner with
//!   `ERROR_SEM_OWNER_DIED`, and the next `DosRequestMutexSem` — or
//!   `DosWaitMuxWaitSem` that takes it — gets the mutex and
//!   `ERROR_SEM_OWNER_DIED`. The reference describes this for a process
//!   end; for a thread that ends while the process goes on, that the next
//!   requester becomes the owner and the state clears is Warpine's reading
//!   (unverified);
//! - `DCE_POSTONE` / `DCE_AUTORESET` (`DosCreateEventSem`, `<bsedos.h>`)
//!   are named by the toolkit headers but described by no public reference
//!   Warpine has; Warpine's reading (unverified): a wait released by a
//!   `DCE_AUTORESET` semaphore resets it, so one post releases one waiter;
//!   with `DCE_POSTONE` every released wait takes one post, so each post
//!   releases one waiter. Other flag bits are `ERROR_INVALID_PARAMETER`.
//!
//! Waking waiters: event and mutex waits block on their semaphore's own
//! condition variable. A muxwait wait cannot block on several, so every
//! change a muxwait list may wait for — a post, a release, an owner that
//! ended, a semaphore deleted, a record added or deleted — also bumps the
//! manager's `SemSignal` generation, and muxwait waiters block on that. A
//! waiter reads the generation before it looks at its list, so a change
//! between the look and the wait is never lost. `SemaphoreManager::wake_all`
//! wakes every waiter (event, mutex, muxwait) so that it looks at its stop
//! conditions again: whatever ends a thread or the process sets its flag
//! and then calls it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::time::Duration;
use super::constants::*;
use super::mutex_ext::MutexExt;
use super::shared_sem::{self, SharedSems};

/// The longest semaphore name, `\SEM32\` included.
const MAX_SEM_NAME: usize = 255;
/// The `DosCreateEventSem` flags.
const EVENT_FLAGS: u32 = DC_SEM_SHARED | DCE_POSTONE | DCE_AUTORESET;

/// A semaphore that counts the threads blocked on it (`ERROR_SEM_BUSY`).
pub trait Waiters {
    fn waiters_mut(&mut self) -> &mut u32;
}

pub struct EventSemaphore {
    /// Posts since the last reset.
    pub post_count: u32,
    /// Its `DCE_POSTONE` / `DCE_AUTORESET` flags.
    pub flags: u32,
    /// Threads in `DosWaitEventSem` on it.
    pub waiters: u32,
}

impl Waiters for EventSemaphore {
    fn waiters_mut(&mut self) -> &mut u32 { &mut self.waiters }
}

impl EventSemaphore {
    pub fn new(posted: bool, flags: u32) -> Self {
        EventSemaphore { post_count: posted as u32, flags: flags & (DCE_POSTONE | DCE_AUTORESET), waiters: 0 }
    }

    pub fn posted(&self) -> bool { self.post_count > 0 }

    /// `DosPostEventSem`: one more post.
    pub fn post(&mut self) -> u32 {
        if self.post_count >= MAX_EVENT_POSTS {
            return ERROR_TOO_MANY_POSTS;
        }
        self.post_count += 1;
        if self.post_count > 1 { ERROR_ALREADY_POSTED } else { NO_ERROR }
    }

    /// `DosResetEventSem`: the post count before the reset.
    pub fn reset(&mut self) -> Result<u32, u32> {
        if self.post_count == 0 {
            return Err(ERROR_ALREADY_RESET);
        }
        Ok(std::mem::take(&mut self.post_count))
    }

    /// A wait was released by the semaphore: `DCE_POSTONE` takes one post,
    /// `DCE_AUTORESET` all of them, a plain event stays posted.
    pub fn consume(&mut self) {
        if self.flags & DCE_POSTONE != 0 {
            self.post_count = self.post_count.saturating_sub(1);
        } else if self.flags & DCE_AUTORESET != 0 {
            self.post_count = 0;
        }
    }
}

pub struct MutexSemaphore {
    /// Warpine vCPU id of the owning thread (`threads::tid_of_vcpu` gives its tid).
    pub owner_vcpu: Option<u32>,
    /// Requests less releases by the owner.
    pub request_count: u32,
    /// The owner ended without releasing the mutex: its thread id, kept
    /// for `DosQueryMutexSem` (its vCPU may run another thread by now).
    pub died_tid: Option<u32>,
    /// Threads in `DosRequestMutexSem` on it.
    pub waiters: u32,
}

impl Waiters for MutexSemaphore {
    fn waiters_mut(&mut self) -> &mut u32 { &mut self.waiters }
}

impl MutexSemaphore {
    pub fn new(owner: Option<u32>) -> Self {
        MutexSemaphore { owner_vcpu: owner, request_count: owner.is_some() as u32, died_tid: None, waiters: 0 }
    }

    /// Whether thread `vcpu` can take the mutex without waiting.
    pub fn available_to(&self, vcpu: u32) -> bool {
        self.died_tid.is_some() || self.owner_vcpu.is_none_or(|o| o == vcpu)
    }

    /// Whether thread `vcpu` owns the mutex (a new thread on a dead owner's
    /// reused vCPU does not).
    pub fn owned_by(&self, vcpu: u32) -> bool {
        self.died_tid.is_none() && self.owner_vcpu == Some(vcpu)
    }

    /// Take (or take again) the mutex for `vcpu`; it must be available.
    /// True when its last owner had died (`ERROR_SEM_OWNER_DIED`).
    pub fn acquire(&mut self, vcpu: u32) -> bool {
        if self.died_tid.take().is_some() {
            self.owner_vcpu = Some(vcpu);
            self.request_count = 1;
            return true;
        }
        if self.owner_vcpu == Some(vcpu) {
            self.request_count += 1;
        } else {
            self.owner_vcpu = Some(vcpu);
            self.request_count = 1;
        }
        false
    }

    /// `DosReleaseMutexSem` by `vcpu`; true when the mutex is free now.
    pub fn release(&mut self, vcpu: u32) -> Result<bool, u32> {
        if !self.owned_by(vcpu) {
            return Err(ERROR_NOT_OWNER);
        }
        self.request_count -= 1;
        if self.request_count == 0 {
            self.owner_vcpu = None;
        }
        Ok(self.request_count == 0)
    }

    /// Thread `vcpu` (thread id `tid`) ended: a mutex it owned goes to the
    /// owner-died state. True when it owned this one.
    pub fn owner_ended(&mut self, vcpu: u32, tid: u32) -> bool {
        if !self.owned_by(vcpu) {
            return false;
        }
        self.died_tid = Some(tid);
        self.request_count = 0;
        true
    }
}

/// The generation counter muxwait waiters block on (see the module notes).
/// Its lock is a leaf: nothing else is locked while it is held.
#[derive(Default)]
pub struct SemSignal {
    generation: Mutex<u64>,
    cond: Condvar,
}

impl SemSignal {
    /// The current generation; read it before looking at the semaphores.
    pub fn generation(&self) -> u64 {
        *self.generation.lock_or_recover()
    }

    /// Something a muxwait list may wait for changed: wake its waiters.
    pub fn bump(&self) {
        let mut g = self.generation.lock_or_recover();
        *g = g.wrapping_add(1);
        self.cond.notify_all();
    }

    /// Block until the generation is no longer `seen` or `timeout` passes.
    pub fn wait_change(&self, seen: u64, timeout: Duration) {
        let g = self.generation.lock_or_recover();
        drop(self.cond.wait_timeout_while(g, timeout, |g| *g == seen)
            .unwrap_or_else(|e| e.into_inner()));
    }
}

/// The kind of a semaphore handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemKind {
    Event,
    Mutex,
    MuxWait,
}

/// One `SEMRECORD` of a muxwait list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxWaitRecord {
    pub hsem: u32,
    pub user: u32,
}

pub struct MuxWaitSemaphore {
    pub records: Vec<MuxWaitRecord>,
    /// The kind of the semaphores in the list (`None` while it is empty).
    pub kind: Option<SemKind>,
    pub wait_all: bool,
    /// Threads in `DosWaitMuxWaitSem` on it.
    pub waiters: u32,
}

pub type EventArc = Arc<(Mutex<EventSemaphore>, Condvar)>;
pub type MutexArc = Arc<(Mutex<MutexSemaphore>, Condvar)>;
pub type MuxArc = Arc<Mutex<MuxWaitSemaphore>>;

/// Counts a thread among a muxwait semaphore's waiters while it lives.
pub struct MuxWaiting(MuxArc);

impl MuxWaiting {
    pub fn new(mux: MuxArc) -> Self {
        mux.lock_or_recover().waiters += 1;
        MuxWaiting(mux)
    }
}

impl Drop for MuxWaiting {
    fn drop(&mut self) {
        let mut m = self.0.lock_or_recover();
        m.waiters = m.waiters.saturating_sub(1);
    }
}

enum SemObject {
    Event(EventArc),
    Mutex(MutexArc),
    MuxWait(MuxArc),
}

impl SemObject {
    fn kind(&self) -> SemKind {
        match self {
            SemObject::Event(_) => SemKind::Event,
            SemObject::Mutex(_) => SemKind::Mutex,
            SemObject::MuxWait(_) => SemKind::MuxWait,
        }
    }

    /// Threads blocked on the semaphore.
    fn waiters(&self) -> u32 {
        match self {
            SemObject::Event(a) => a.0.lock_or_recover().waiters,
            SemObject::Mutex(a) => a.0.lock_or_recover().waiters,
            SemObject::MuxWait(a) => a.lock_or_recover().waiters,
        }
    }
}

struct SemEntry {
    object: SemObject,
    /// Upper-cased name (names are not case-sensitive, like file names).
    name: Option<String>,
    attr: u32,
    /// Usage count: the create plus every open, less the closes.
    opens: u32,
}

pub struct SemaphoreManager {
    sems: HashMap<u32, SemEntry>,
    next_handle: u32,
    /// Wakes muxwait waiters; cloned out and waited on without this
    /// manager's lock.
    pub signal: Arc<SemSignal>,
    /// The session's shared semaphores (`shared_sem.rs`): named and
    /// `DC_SEM_SHARED` events and mutexes, handles with `SHARED_SEM_BIT`.
    pub shared: Option<Arc<SharedSems>>,
}

/// The canonical form of a semaphore name: upper case, `\SEM32\…`, not
/// too long (`ERROR_INVALID_NAME`).
pub fn canonical_sem_name(name: &str) -> Result<String, u32> {
    let upper = name.to_ascii_uppercase();
    if !upper.starts_with(SEM32_PREFIX) || upper.len() == SEM32_PREFIX.len() || upper.len() > MAX_SEM_NAME {
        return Err(ERROR_INVALID_NAME);
    }
    Ok(upper)
}

impl Default for SemaphoreManager {
    fn default() -> Self { Self::new() }
}

impl SemaphoreManager {
    pub fn new() -> Self {
        SemaphoreManager { sems: HashMap::new(), next_handle: 1, signal: Arc::default(), shared: None }
    }

    /// Something a waiter may wait for changed: wake muxwait waiters here
    /// and, for lists that hold shared semaphores, on the session's word.
    fn bump(&self) {
        self.signal.bump();
        if let Some(s) = &self.shared { s.notify(); }
    }

    /// Whether a semaphore of this process has the (canonical) name.
    pub fn local_name_exists(&self, name: &str) -> bool {
        self.sems.values().any(|e| e.name.as_deref() == Some(name))
    }

    /// Check a new semaphore's name: `\SEM32\…`, not too long, not in use
    /// here or by a shared semaphore of the session.
    fn check_new_name(&self, name: Option<&str>) -> Result<Option<String>, u32> {
        let Some(name) = name else { return Ok(None) };
        let upper = canonical_sem_name(name)?;
        if self.local_name_exists(&upper) || self.shared.as_ref().is_some_and(|s| s.name_exists(&upper)) {
            return Err(ERROR_DUPLICATE_NAME);
        }
        Ok(Some(upper))
    }

    fn insert(&mut self, name: Option<&str>, attr: u32, object: SemObject) -> Result<u32, u32> {
        let name = self.check_new_name(name)?;
        let h = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(ERROR_TOO_MANY_HANDLES)?;
        self.sems.insert(h, SemEntry { object, name, attr, opens: 1 });
        Ok(h)
    }

    /// `DosCreateEventSem`: `DC_SEM_SHARED`, `DCE_POSTONE`, `DCE_AUTORESET`.
    pub fn create_event(&mut self, name: Option<&str>, attr: u32, posted: bool) -> Result<u32, u32> {
        if attr & !EVENT_FLAGS != 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let sem = EventSemaphore::new(posted, attr);
        self.insert(name, attr, SemObject::Event(Arc::new((Mutex::new(sem), Condvar::new()))))
    }

    /// `DosCreateMutexSem`; `owner` is the creating thread's vCPU when it
    /// is created owned.
    pub fn create_mutex(&mut self, name: Option<&str>, attr: u32, owner: Option<u32>) -> Result<u32, u32> {
        let sem = MutexSemaphore::new(owner);
        self.insert(name, attr, SemObject::Mutex(Arc::new((Mutex::new(sem), Condvar::new()))))
    }

    /// `DosCreateMuxWaitSem`.
    pub fn create_mux(&mut self, name: Option<&str>, attr: u32, records: &[MuxWaitRecord]) -> Result<u32, u32> {
        let wait_all = check_mux_attr(attr)?;
        let kind = self.check_records(None, &[], records)?;
        let mux = MuxWaitSemaphore { records: records.to_vec(), kind, wait_all, waiters: 0 };
        self.insert(name, attr, SemObject::MuxWait(Arc::new(Mutex::new(mux))))
    }

    /// The kind of the semaphores of `records` once they join a list of
    /// `existing` records of `kind`, or why they cannot.
    fn check_records(&self, kind: Option<SemKind>, existing: &[MuxWaitRecord], records: &[MuxWaitRecord])
        -> Result<Option<SemKind>, u32> {
        check_mux_records(kind, existing, records, |h| self.kind_of(h))
    }

    pub fn kind_of(&self, h: u32) -> Option<SemKind> {
        if shared_sem::is_shared(h) {
            return self.shared.as_ref()?.kind_of(h);
        }
        self.sems.get(&h).map(|e| e.object.kind())
    }

    /// Whether `h` is an event semaphore (of this process or shared).
    pub fn is_event(&self, h: u32) -> bool {
        self.kind_of(h) == Some(SemKind::Event)
    }

    pub fn get_event(&self, h: u32) -> Option<EventArc> {
        match self.sems.get(&h).map(|e| &e.object) {
            Some(SemObject::Event(a)) => Some(Arc::clone(a)),
            _ => None,
        }
    }

    /// Whether `h` is a shared event semaphore: created with
    /// `DC_SEM_SHARED` or with a name (named semaphores are shared).
    pub fn is_shared_event(&self, h: u32) -> bool {
        if shared_sem::is_shared(h) {
            return self.is_event(h);
        }
        self.sems.get(&h).is_some_and(|e| matches!(e.object, SemObject::Event(_))
            && (e.attr & DC_SEM_SHARED != 0 || e.name.is_some()))
    }

    pub fn get_mutex(&self, h: u32) -> Option<MutexArc> {
        match self.sems.get(&h).map(|e| &e.object) {
            Some(SemObject::Mutex(a)) => Some(Arc::clone(a)),
            _ => None,
        }
    }

    pub fn get_mux(&self, h: u32) -> Option<MuxArc> {
        match self.sems.get(&h).map(|e| &e.object) {
            Some(SemObject::MuxWait(a)) => Some(Arc::clone(a)),
            _ => None,
        }
    }

    /// `DosOpen*Sem`: by `name`, else by handle `h`; the handle opened.
    pub fn open(&mut self, kind: SemKind, name: Option<&str>, h: u32) -> Result<u32, u32> {
        let h = match name {
            Some(name) => {
                let upper = name.to_ascii_uppercase();
                self.sems.iter()
                    .find(|(_, e)| e.name.as_deref() == Some(upper.as_str()) && e.object.kind() == kind)
                    .map(|(&h, _)| h)
                    .ok_or(ERROR_SEM_NOT_FOUND)?
            }
            None => h,
        };
        let entry = self.sems.get_mut(&h)
            .filter(|e| e.object.kind() == kind)
            .ok_or(ERROR_INVALID_HANDLE)?;
        if entry.opens >= MAX_SEM_OPENS {
            return Err(ERROR_TOO_MANY_OPENS);
        }
        entry.opens += 1;
        Ok(h)
    }

    /// `DosClose*Sem`: one use less; the last one deletes the semaphore.
    /// `ERROR_SEM_BUSY` while another thread is blocked on it.
    pub fn close(&mut self, kind: SemKind, h: u32) -> u32 {
        let Some(entry) = self.sems.get_mut(&h).filter(|e| e.object.kind() == kind) else {
            return ERROR_INVALID_HANDLE;
        };
        if entry.object.waiters() > 0 {
            return ERROR_SEM_BUSY;
        }
        entry.opens -= 1;
        if entry.opens == 0 {
            self.sems.remove(&h);
            // A muxwait wait on a list that holds it fails now.
            self.bump();
        }
        NO_ERROR
    }

    /// `DosPostEventSem` (and the posts of timers and pipes): one post,
    /// waiters woken.
    pub fn post_event(&self, h: u32) -> u32 {
        if shared_sem::is_shared(h) {
            let rc = self.shared.as_ref().map_or(ERROR_INVALID_HANDLE, |s| s.post(h));
            if rc != ERROR_INVALID_HANDLE { self.signal.bump(); }
            return rc;
        }
        let Some(sem) = self.get_event(h) else { return ERROR_INVALID_HANDLE };
        let rc = {
            let (lock, cvar) = &*sem;
            let rc = lock.lock_or_recover().post();
            cvar.notify_all();
            rc
        };
        self.bump();
        rc
    }

    /// `DosReleaseMutexSem` by thread `vcpu`.
    pub fn release_mutex(&self, h: u32, vcpu: u32) -> u32 {
        let Some(sem) = self.get_mutex(h) else { return ERROR_INVALID_HANDLE };
        let freed = match sem.0.lock_or_recover().release(vcpu) {
            Ok(freed) => freed,
            Err(e) => return e,
        };
        if freed {
            sem.1.notify_all();
            self.bump();
        }
        NO_ERROR
    }

    /// Thread `vcpu` (thread id `tid`) ended: every mutex it owned goes to
    /// the owner-died state and its waiters are woken. How many it owned.
    pub fn owner_ended(&self, vcpu: u32, tid: u32) -> usize {
        if let Some(s) = &self.shared { s.owner_ended(tid); }
        let mut n = 0;
        for e in self.sems.values() {
            if let SemObject::Mutex(a) = &e.object && a.0.lock_or_recover().owner_ended(vcpu, tid) {
                n += 1;
                a.1.notify_all();
            }
        }
        if n > 0 {
            self.bump();
        }
        n
    }

    /// Wake every thread blocked on a semaphore of this process, so that it
    /// looks at its stop conditions again (a thread or the process ending:
    /// set the flag first, then call this). Each semaphore's lock is held
    /// while it is notified, so a waiter between its check and its wait is
    /// not missed.
    pub fn wake_all(&self) {
        for e in self.sems.values() {
            match &e.object {
                SemObject::Event(a) => { let _g = a.0.lock_or_recover(); a.1.notify_all(); }
                SemObject::Mutex(a) => { let _g = a.0.lock_or_recover(); a.1.notify_all(); }
                SemObject::MuxWait(_) => {}
            }
        }
        self.bump();
    }

    /// `DosAddMuxWaitSem`.
    pub fn add_mux_record(&self, hmux: u32, rec: MuxWaitRecord) -> u32 {
        let Some(mux) = self.get_mux(hmux) else { return ERROR_INVALID_HANDLE };
        let mut mux = mux.lock_or_recover();
        match self.check_records(mux.kind, &mux.records, &[rec]) {
            Ok(kind) => {
                mux.kind = kind;
                mux.records.push(rec);
                drop(mux);
                self.bump();
                NO_ERROR
            }
            Err(e) => e,
        }
    }

    /// `DosDeleteMuxWaitSem`.
    pub fn delete_mux_record(&self, hmux: u32, hsem: u32) -> u32 {
        let Some(mux) = self.get_mux(hmux) else { return ERROR_INVALID_HANDLE };
        let mut mux = mux.lock_or_recover();
        if mux.records.is_empty() {
            return ERROR_EMPTY_MUXWAIT;
        }
        let Some(pos) = mux.records.iter().position(|r| r.hsem == hsem) else {
            return ERROR_INVALID_HANDLE;
        };
        mux.records.remove(pos);
        if mux.records.is_empty() {
            mux.kind = None;
        }
        drop(mux);
        // A wait for all of them may be complete now.
        self.bump();
        NO_ERROR
    }

    /// `DosQueryMuxWaitSem`: the records and the attribute flags (a named
    /// semaphore is shared).
    pub fn query_mux(&self, hmux: u32) -> Result<(Vec<MuxWaitRecord>, u32), u32> {
        let entry = self.sems.get(&hmux).ok_or(ERROR_INVALID_HANDLE)?;
        let SemObject::MuxWait(mux) = &entry.object else { return Err(ERROR_INVALID_HANDLE) };
        let shared = if entry.name.is_some() { DC_SEM_SHARED } else { 0 };
        Ok((mux.lock_or_recover().records.clone(), entry.attr | shared))
    }

    /// One look at muxwait semaphore `hmux` for thread `vcpu`: the user
    /// value of the record that ends the wait and whether a mutex it took
    /// had lost its owner (`ERROR_SEM_OWNER_DIED`), or `None` to wait on.
    /// The mutexes waited for are taken, and `DCE_POSTONE` /
    /// `DCE_AUTORESET` events that end the wait consume their posts.
    /// Waiting for any semaphore takes the first ready one in list order;
    /// waiting for all returns the last record's user value.
    pub fn try_mux(&self, hmux: u32, vcpu: u32) -> Result<Option<(u32, bool)>, u32> {
        let mux = self.get_mux(hmux).ok_or(ERROR_INVALID_HANDLE)?;
        let mux = mux.lock_or_recover();
        let Some(last) = mux.records.last() else { return Err(ERROR_EMPTY_MUXWAIT) };
        if mux.records.iter().any(|r| shared_sem::is_shared(r.hsem)) {
            let shared = self.shared.as_ref().ok_or(ERROR_INVALID_HANDLE)?;
            return shared.with_many(|m| self.try_mixed_mux(&mux, m, vcpu));
        }
        if mux.kind == Some(SemKind::Mutex) {
            let arcs = mux.records.iter()
                .map(|r| self.get_mutex(r.hsem).ok_or(ERROR_INVALID_HANDLE))
                .collect::<Result<Vec<_>, _>>()?;
            let mut guards: Vec<_> = arcs.iter().map(|a| a.0.lock_or_recover()).collect();
            if mux.wait_all {
                if !guards.iter().all(|g| g.available_to(vcpu)) { return Ok(None) }
                let died = guards.iter_mut().fold(false, |died, g| g.acquire(vcpu) | died);
                return Ok(Some((last.user, died)));
            }
            let Some(i) = guards.iter().position(|g| g.available_to(vcpu)) else { return Ok(None) };
            let died = guards[i].acquire(vcpu);
            return Ok(Some((mux.records[i].user, died)));
        }
        let arcs = mux.records.iter()
            .map(|r| self.get_event(r.hsem).ok_or(ERROR_INVALID_HANDLE))
            .collect::<Result<Vec<_>, _>>()?;
        let mut guards: Vec<_> = arcs.iter().map(|a| a.0.lock_or_recover()).collect();
        if mux.wait_all {
            if !guards.iter().all(|g| g.posted()) { return Ok(None) }
            guards.iter_mut().for_each(|g| g.consume());
            return Ok(Some((last.user, false)));
        }
        let Some(i) = guards.iter().position(|g| g.posted()) else { return Ok(None) };
        guards[i].consume();
        Ok(Some((mux.records[i].user, false)))
    }
}

impl SemaphoreManager {
    /// `try_mux` for a list holding shared semaphores: all looked at (and
    /// taken) under the session's lock.
    fn try_mixed_mux(&self, mux: &MuxWaitSemaphore, m: &mut shared_sem::Many, vcpu: u32)
        -> Result<Option<(u32, bool)>, u32>
    {
        let tid = super::threads::tid_of_vcpu(vcpu);
        let last = mux.records.last().ok_or(ERROR_EMPTY_MUXWAIT)?;
        // Local members are locked for the whole look, like `try_mux`.
        enum Member<'g> {
            LocalEvent(std::sync::MutexGuard<'g, EventSemaphore>),
            LocalMutex(std::sync::MutexGuard<'g, MutexSemaphore>),
            Shared(u32),
        }
        let events: Vec<Option<EventArc>> = mux.records.iter().map(|r| self.get_event(r.hsem)).collect();
        let mutexes: Vec<Option<MutexArc>> = mux.records.iter().map(|r| self.get_mutex(r.hsem)).collect();
        let want = mux.kind.ok_or(ERROR_EMPTY_MUXWAIT)?;
        let mut members = Vec::with_capacity(mux.records.len());
        for (i, r) in mux.records.iter().enumerate() {
            members.push(if shared_sem::is_shared(r.hsem) {
                if m.kind_of(r.hsem) != Some(want) { return Err(ERROR_INVALID_HANDLE); }
                Member::Shared(r.hsem)
            } else if want == SemKind::Event {
                Member::LocalEvent(events[i].as_ref().ok_or(ERROR_INVALID_HANDLE)?.0.lock_or_recover())
            } else {
                Member::LocalMutex(mutexes[i].as_ref().ok_or(ERROR_INVALID_HANDLE)?.0.lock_or_recover())
            });
        }
        let ready = |mem: &Member, m: &shared_sem::Many| match mem {
            Member::LocalEvent(g) => g.posted(),
            Member::LocalMutex(g) => g.available_to(vcpu),
            Member::Shared(h) => if want == SemKind::Event { m.event_posted(*h) == Some(true) }
                                 else { m.mutex_available(*h, tid) == Some(true) },
        };
        let take = |mem: &mut Member, m: &mut shared_sem::Many| -> bool {
            match mem {
                Member::LocalEvent(g) => { g.consume(); false }
                Member::LocalMutex(g) => g.acquire(vcpu),
                Member::Shared(h) => if want == SemKind::Event { m.event_consume(*h); false }
                                     else { m.mutex_acquire(*h, tid) },
            }
        };
        if mux.wait_all {
            if !members.iter().all(|mem| ready(mem, m)) { return Ok(None); }
            let died = members.iter_mut().fold(false, |d, mem| take(mem, m) | d);
            return Ok(Some((last.user, died)));
        }
        let Some(i) = members.iter().position(|mem| ready(mem, m)) else { return Ok(None) };
        let died = take(&mut members[i], m);
        Ok(Some((mux.records[i].user, died)))
    }
}

/// The kind of the semaphores of `records` once they join a list of
/// `existing` records of `kind`, or why they cannot: at most
/// `MAX_MUXWAIT_SEMS`, each a known event or mutex (`kind_of`), all of one
/// kind, each once.
pub fn check_mux_records(kind: Option<SemKind>, existing: &[MuxWaitRecord], records: &[MuxWaitRecord],
                         kind_of: impl Fn(u32) -> Option<SemKind>) -> Result<Option<SemKind>, u32> {
    if existing.len() + records.len() > MAX_MUXWAIT_SEMS {
        return Err(ERROR_TOO_MANY_SEMAPHORES);
    }
    let mut kind = kind;
    for (i, rec) in records.iter().enumerate() {
        let k = kind_of(rec.hsem).ok_or(ERROR_INVALID_HANDLE)?;
        if k == SemKind::MuxWait || kind.is_some_and(|kind| kind != k) {
            return Err(ERROR_WRONG_TYPE);
        }
        kind = Some(k);
        if existing.iter().chain(&records[..i]).any(|r| r.hsem == rec.hsem) {
            return Err(ERROR_DUPLICATE_HANDLE);
        }
    }
    Ok(kind)
}

/// `DosCreateMuxWaitSem` flags: whether the list waits for all its
/// semaphores. Exactly one of `DCMW_WAIT_ANY` / `DCMW_WAIT_ALL`, beside
/// `DC_SEM_SHARED`.
pub fn check_mux_attr(attr: u32) -> Result<bool, u32> {
    if attr & !(DC_SEM_SHARED | DCMW_WAIT_ANY | DCMW_WAIT_ALL) != 0 {
        return Err(ERROR_INVALID_PARAMETER);
    }
    match attr & (DCMW_WAIT_ANY | DCMW_WAIT_ALL) {
        DCMW_WAIT_ANY => Ok(false),
        DCMW_WAIT_ALL => Ok(true),
        _ => Err(ERROR_INVALID_PARAMETER),
    }
}

pub struct QueueEntry {
    pub data: Vec<u8>,
    pub event: u32,
    pub priority: u32,
    /// Its element code (DosPeekQueue, DosReadQueue), unique in its queue.
    pub id: u32,
}

/// The largest element `DosWriteQueue` copies.
pub const MAX_QUEUE_ELEMENT: u32 = 64 * 1024 * 1024;
/// The most elements, and bytes, one queue holds (Warpine's limits; OS/2
/// bounds a queue by the memory its writers share).
pub const MAX_QUEUE_ITEMS: usize = 1 << 16;
pub const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;

pub struct OS2Queue {
    pub name: String,
    pub items: VecDeque<QueueEntry>,
    pub attr: u32,
    /// The next element code.
    pub next_id: u32,
    pub cond: Arc<Condvar>,
    pub cond_lock: Arc<Mutex<bool>>,
}

pub struct QueueManager {
    queues: HashMap<u32, Arc<Mutex<OS2Queue>>>,
    next_handle: u32,
}

impl Default for QueueManager {
    fn default() -> Self { Self::new() }
}

impl QueueManager {
    pub fn new() -> Self {
        QueueManager { queues: HashMap::new(), next_handle: 1 }
    }
    pub fn create(&mut self, name: String, attr: u32) -> u32 {
        let h = self.next_handle;
        self.queues.insert(h, Arc::new(Mutex::new(OS2Queue {
            name, items: VecDeque::new(), attr, next_id: 1,
            cond: Arc::new(Condvar::new()), cond_lock: Arc::new(Mutex::new(false)),
        })));
        self.next_handle += 1;
        h
    }
    pub fn get(&self, h: u32) -> Option<Arc<Mutex<OS2Queue>>> {
        self.queues.get(&h).cloned()
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.queues.remove(&h).is_some()
    }
    pub fn find_by_name(&self, name: &str) -> Option<u32> {
        for (&h, q_arc) in &self.queues {
            if q_arc.lock_or_recover().name == name {
                return Some(h);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn signal_wait_ends_on_a_bump() {
        let signal = Arc::new(SemSignal::default());
        let seen = signal.generation();
        let s2 = signal.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            s2.wait_change(seen, Duration::from_secs(30));
            start.elapsed()
        });
        std::thread::sleep(Duration::from_millis(20));
        signal.bump();
        assert!(t.join().unwrap() < Duration::from_secs(10));
        // A bump between the look and the wait is not lost.
        let seen = signal.generation();
        signal.bump();
        let start = Instant::now();
        signal.wait_change(seen, Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn changes_bump_the_signal() {
        let mut sm = SemaphoreManager::new();
        let ev = sm.create_event(None, 0, false).unwrap();
        let m = sm.create_mutex(None, 0, Some(1)).unwrap();
        let mux = sm.create_mux(None, DCMW_WAIT_ANY, &[MuxWaitRecord { hsem: ev, user: 1 }]).unwrap();
        let mut g = sm.signal.generation();
        let mut bumped = |sm: &SemaphoreManager| {
            let now = sm.signal.generation();
            let b = now != g;
            g = now;
            b
        };
        sm.post_event(ev);
        assert!(bumped(&sm), "post");
        assert_eq!(sm.release_mutex(m, 2), ERROR_NOT_OWNER);
        assert!(!bumped(&sm), "a failed release");
        assert_eq!(sm.release_mutex(m, 1), NO_ERROR);
        assert!(bumped(&sm), "release");
        sm.delete_mux_record(mux, ev);
        assert!(bumped(&sm), "record deleted");
        sm.add_mux_record(mux, MuxWaitRecord { hsem: ev, user: 1 });
        assert!(bumped(&sm), "record added");
        sm.wake_all();
        assert!(bumped(&sm), "wake_all");
        assert_eq!(sm.owner_ended(1, 1), 0);
        assert!(!bumped(&sm), "no mutex owned");
        assert_eq!(sm.close(SemKind::MuxWait, mux), NO_ERROR);
        assert!(bumped(&sm), "deleted");
    }

    #[test]
    fn mutex_owner_died_state() {
        let mut m = MutexSemaphore::new(Some(7));
        assert!(!m.acquire(7));
        assert_eq!(m.request_count, 2);
        assert!(m.owner_ended(7, 7));
        assert!(!m.owner_ended(7, 7), "only once");
        assert!(m.available_to(9) && !m.owned_by(7));
        assert_eq!(m.release(7), Err(ERROR_NOT_OWNER));
        assert!(m.acquire(9), "the next owner hears of it");
        assert!(!m.acquire(9));
        assert_eq!(m.release(9), Ok(false));
        assert_eq!(m.release(9), Ok(true));
        assert_eq!(m.owner_vcpu, None);
    }
}
