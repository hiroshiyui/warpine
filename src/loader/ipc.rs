// SPDX-License-Identifier: GPL-3.0-only
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
//!   close deletes the semaphore.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use super::constants::*;
use super::mutex_ext::MutexExt;

/// The longest semaphore name, `\SEM32\` included.
const MAX_SEM_NAME: usize = 255;

pub struct EventSemaphore {
    /// Posts since the last reset.
    pub post_count: u32,
}

impl EventSemaphore {
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
}

pub struct MutexSemaphore {
    /// Warpine vCPU id of the owning thread (`threads::tid_of_vcpu` gives its tid).
    pub owner_vcpu: Option<u32>,
    /// Requests less releases by the owner.
    pub request_count: u32,
}

impl MutexSemaphore {
    /// Whether thread `vcpu` can take the mutex without waiting.
    pub fn available_to(&self, vcpu: u32) -> bool {
        self.owner_vcpu.is_none_or(|o| o == vcpu)
    }

    /// Take (or take again) the mutex for `vcpu`; it must be available.
    pub fn acquire(&mut self, vcpu: u32) {
        if self.owner_vcpu == Some(vcpu) {
            self.request_count += 1;
        } else {
            self.owner_vcpu = Some(vcpu);
            self.request_count = 1;
        }
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
}

pub type EventArc = Arc<(Mutex<EventSemaphore>, Condvar)>;
pub type MutexArc = Arc<(Mutex<MutexSemaphore>, Condvar)>;
pub type MuxArc = Arc<Mutex<MuxWaitSemaphore>>;

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
}

impl Default for SemaphoreManager {
    fn default() -> Self { Self::new() }
}

impl SemaphoreManager {
    pub fn new() -> Self {
        SemaphoreManager { sems: HashMap::new(), next_handle: 1 }
    }

    /// Check a new semaphore's name: `\SEM32\…`, not too long, not in use.
    fn check_new_name(&self, name: Option<&str>) -> Result<Option<String>, u32> {
        let Some(name) = name else { return Ok(None) };
        let upper = name.to_ascii_uppercase();
        if !upper.starts_with(SEM32_PREFIX) || upper.len() == SEM32_PREFIX.len()
            || upper.len() > MAX_SEM_NAME {
            return Err(ERROR_INVALID_NAME);
        }
        if self.sems.values().any(|e| e.name.as_deref() == Some(upper.as_str())) {
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

    /// `DosCreateEventSem`.
    pub fn create_event(&mut self, name: Option<&str>, attr: u32, posted: bool) -> Result<u32, u32> {
        let sem = EventSemaphore { post_count: posted as u32 };
        self.insert(name, attr, SemObject::Event(Arc::new((Mutex::new(sem), Condvar::new()))))
    }

    /// `DosCreateMutexSem`; `owner` is the creating thread's vCPU when it
    /// is created owned.
    pub fn create_mutex(&mut self, name: Option<&str>, attr: u32, owner: Option<u32>) -> Result<u32, u32> {
        let sem = MutexSemaphore { owner_vcpu: owner, request_count: owner.is_some() as u32 };
        self.insert(name, attr, SemObject::Mutex(Arc::new((Mutex::new(sem), Condvar::new()))))
    }

    /// `DosCreateMuxWaitSem`.
    pub fn create_mux(&mut self, name: Option<&str>, attr: u32, records: &[MuxWaitRecord]) -> Result<u32, u32> {
        let wait_all = check_mux_attr(attr)?;
        let kind = self.check_records(None, &[], records)?;
        let mux = MuxWaitSemaphore { records: records.to_vec(), kind, wait_all };
        self.insert(name, attr, SemObject::MuxWait(Arc::new(Mutex::new(mux))))
    }

    /// The kind of the semaphores of `records` once they join a list of
    /// `existing` records of `kind`, or why they cannot.
    fn check_records(&self, kind: Option<SemKind>, existing: &[MuxWaitRecord], records: &[MuxWaitRecord])
        -> Result<Option<SemKind>, u32> {
        if existing.len() + records.len() > MAX_MUXWAIT_SEMS {
            return Err(ERROR_TOO_MANY_SEMAPHORES);
        }
        let mut kind = kind;
        for (i, rec) in records.iter().enumerate() {
            let k = self.kind_of(rec.hsem).ok_or(ERROR_INVALID_HANDLE)?;
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

    pub fn kind_of(&self, h: u32) -> Option<SemKind> {
        self.sems.get(&h).map(|e| e.object.kind())
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
    pub fn close(&mut self, kind: SemKind, h: u32) -> u32 {
        let Some(entry) = self.sems.get_mut(&h).filter(|e| e.object.kind() == kind) else {
            return ERROR_INVALID_HANDLE;
        };
        entry.opens -= 1;
        if entry.opens == 0 {
            self.sems.remove(&h);
        }
        NO_ERROR
    }

    /// `DosAddMuxWaitSem`.
    pub fn add_mux_record(&self, hmux: u32, rec: MuxWaitRecord) -> u32 {
        let Some(mux) = self.get_mux(hmux) else { return ERROR_INVALID_HANDLE };
        let mut mux = mux.lock_or_recover();
        match self.check_records(mux.kind, &mux.records, &[rec]) {
            Ok(kind) => {
                mux.kind = kind;
                mux.records.push(rec);
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
    /// value of the record that ends the wait (the mutexes waited for are
    /// then taken), or `None` to wait on. Waiting for any semaphore takes
    /// the first ready one in list order; waiting for all returns the last
    /// record's user value.
    pub fn try_mux(&self, hmux: u32, vcpu: u32) -> Result<Option<u32>, u32> {
        let mux = self.get_mux(hmux).ok_or(ERROR_INVALID_HANDLE)?;
        let mux = mux.lock_or_recover();
        let Some(last) = mux.records.last() else { return Err(ERROR_EMPTY_MUXWAIT) };
        if mux.kind == Some(SemKind::Mutex) {
            let arcs = mux.records.iter()
                .map(|r| self.get_mutex(r.hsem).ok_or(ERROR_INVALID_HANDLE))
                .collect::<Result<Vec<_>, _>>()?;
            let mut guards: Vec<_> = arcs.iter().map(|a| a.0.lock_or_recover()).collect();
            if mux.wait_all {
                if !guards.iter().all(|g| g.available_to(vcpu)) { return Ok(None) }
                guards.iter_mut().for_each(|g| g.acquire(vcpu));
                return Ok(Some(last.user));
            }
            let Some(i) = guards.iter().position(|g| g.available_to(vcpu)) else { return Ok(None) };
            guards[i].acquire(vcpu);
            return Ok(Some(mux.records[i].user));
        }
        let posted = mux.records.iter()
            .map(|r| self.get_event(r.hsem).ok_or(ERROR_INVALID_HANDLE)
                .map(|a| a.0.lock_or_recover().posted()))
            .collect::<Result<Vec<_>, _>>()?;
        if mux.wait_all {
            Ok(posted.iter().all(|&p| p).then_some(last.user))
        } else {
            Ok(posted.iter().position(|&p| p).map(|i| mux.records[i].user))
        }
    }
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
}

pub struct OS2Queue {
    pub name: String,
    pub items: VecDeque<QueueEntry>,
    pub attr: u32,
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
            name, items: VecDeque::new(), attr,
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
