// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Page states of the memory a program allocates (ADR-0114).
//!
//! `DosAllocMem` reserves pages and, with `PAG_COMMIT`, commits them with
//! the access it asks for; `DosSetMem` commits, decommits and changes the
//! access of pages; `DosQueryMem` reports them; `DosFreeMem` gives them
//! back (Control Program Programming Reference). A page's state lives in
//! a [`PageMap`] region and in its entry of the process tables
//! (`paging.rs`), which every thread of the process shares:
//! - committed with write access: present and writable;
//! - committed, read or execute access only: present, read-only;
//! - reserved (not committed), or a guard page: not present.
//!
//! A guard page touched loses its guard and raises
//! `XCPT_GUARD_PAGE_VIOLATION` (continuable); a reserved or read-only page
//! touched the wrong way raises `XCPT_ACCESS_VIOLATION`. Memory a program
//! allocates with `DosAllocMem` has these states; the allocator's other
//! blocks (Warpine's own allocations) are present and writable while
//! allocated, and free pages are not present (`sync_allocations`).
//!
//! When a page loses access, other vCPUs may still hold the old entry in
//! their TLB: [`super::Loader::flush_tlbs`] counts an epoch up and kicks
//! every vCPU, and each reloads CR3 before it runs guest code again.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::paging::{process_pte, process_pte_at, PageAccess};
use log::debug;

/// The flags `DosAllocMem` takes.
const ALLOC_FLAGS: u32 = FPERM | PAG_GUARD | PAG_COMMIT | OBJ_TILE | OBJ_ANY;
/// The flags `DosSetMem` takes (`fSET`, and `PAG_GUARD`).
const SET_FLAGS: u32 = PAG_COMMIT | PAG_DECOMMIT | PAG_DEFAULT | FPERM | PAG_GUARD;
/// What a page's state keeps: its access, guard and commit bits.
const STATE_BITS: u32 = FPERM | PAG_GUARD | PAG_COMMIT;

/// A page whose process-table entry changes: its address and new access.
pub type Change = (u32, PageAccess);

/// An LX object's write flag (IBM's LX format description: `OBJWRITE`).
pub const LX_OBJECT_WRITE: u32 = 0x0002;
/// An LX object's execute flag (`OBJEXEC`).
pub const LX_OBJECT_EXEC: u32 = 0x0004;

/// The main thread's stack (thread 1): from `bottom` up to `top`, the
/// entry ESP, in the program's stack object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainStack {
    pub bottom: u32,
    pub top: u32,
}

impl MainStack {
    /// The stack of a program whose stack object (`object_base`,
    /// `object_bytes`) has the entry ESP at `top` and whose header gives
    /// `stack_size` (`STACKSIZE`): the `stack_size` bytes below `top`,
    /// not below the object. None: no stack size, or a `top` outside
    /// the object.
    pub fn of(object_base: u32, object_bytes: u32, top: u32, stack_size: u32) -> Option<Self> {
        let object_end = u64::from(object_base) + u64::from(object_bytes);
        if stack_size == 0 || top <= object_base || u64::from(top) > object_end {
            return None;
        }
        Some(Self { bottom: top.saturating_sub(stack_size).max(object_base), top })
    }

    /// The whole pages of the stack that can be sparse: from the first
    /// page above the bottom (a page shared with the object's data stays
    /// committed) through the page that holds the entry ESP. `(base,
    /// bytes)`; no bytes when there are none.
    pub fn sparse_pages(&self) -> (u32, u32) {
        let up = |a: u32| a.div_ceil(PAGE_SIZE).saturating_mul(PAGE_SIZE);
        let base = up(self.bottom);
        // The page holding the bytes just below the entry ESP.
        let end = up(self.top);
        (base, end.saturating_sub(base))
    }
}

/// The pages of `objects` (`(base, bytes, LX flags)`) that are read-only:
/// those of an object without the write flag, but for a page it shares
/// with a writable object.
pub fn read_only_pages(objects: &[(u32, u32, u32)]) -> Vec<u32> {
    let pages = |base: u32, bytes: u32| {
        let first = base / PAGE_SIZE;
        // At most the last page of the 32-bit space.
        const PAGES_IN_4GB: u64 = 1 << 20;
        let end = (u64::from(base) + u64::from(bytes)).div_ceil(u64::from(PAGE_SIZE)).min(PAGES_IN_4GB) as u32;
        first..end.max(first)
    };
    let writable: Vec<_> = objects.iter().filter(|o| o.2 & LX_OBJECT_WRITE != 0).map(|o| pages(o.0, o.1)).collect();
    let mut out: Vec<u32> = objects.iter().filter(|o| o.2 & LX_OBJECT_WRITE == 0)
        .flat_map(|o| pages(o.0, o.1))
        .filter(|p| !writable.iter().any(|w| w.contains(p)))
        .map(|p| p * PAGE_SIZE)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One `DosAllocMem` allocation, or a thread's sparse stack: each page's
/// state (`PAG_*` bits), and the access it was allocated with
/// (`PAG_DEFAULT`).
#[derive(Debug, Clone)]
struct Region {
    pages: Vec<u32>,
    default: u32,
    /// A thread's sparse stack: it grows when touched below its committed
    /// part.
    stack: bool,
    /// `XCPT_UNABLE_TO_GROW_STACK` was raised for it already.
    exhausted: bool,
}

/// Stacks of fewer pages are committed whole.
pub const MIN_SPARSE_STACK_PAGES: u32 = 4;
/// The bottom pages of a sparse stack: touching them raises
/// `XCPT_UNABLE_TO_GROW_STACK` (once), leaving them for the handler.
const STACK_RESERVE_PAGES: usize = 2;

impl Region {
    fn end(&self, base: u32) -> u64 {
        u64::from(base) + self.pages.len() as u64 * u64::from(PAGE_SIZE)
    }
}

/// What a page in `state` lets the guest do: executable only with
/// `PAG_EXECUTE` (ADR-0131).
fn access_of(state: u32) -> PageAccess {
    if state & PAG_COMMIT == 0 || state & PAG_GUARD != 0 || state & FPERM == 0 {
        PageAccess::None
    } else {
        PageAccess::of(true, state & PAG_WRITE != 0, state & PAG_EXECUTE != 0)
    }
}

/// Whether going from `old` to `new` takes access away (a TLB may hold
/// the old entry): presence, write or execute permission.
fn loses_access(old: PageAccess, new: PageAccess) -> bool {
    old.loses_to(new)
}

/// What `DosSetMem` changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SetOutcome {
    pub changes: Vec<Change>,
    /// Pages committed now, which start zeroed.
    pub committed: Vec<u32>,
    /// Some page lost access: TLBs must be flushed.
    pub downgrade: bool,
}

/// What a page fault in tracked memory means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// The page allows the access now (a TLB held an older entry): run
    /// the instruction again.
    Retry,
    /// A guard page, now without its guard: `XCPT_GUARD_PAGE_VIOLATION`.
    Guard,
    /// A thread's stack grew to its bottom pages, which are committed
    /// now: `XCPT_UNABLE_TO_GROW_STACK`, once.
    StackExhausted,
    /// Anything else: an access violation.
    Violation,
}

/// The page states of every `DosAllocMem` allocation of the process.
#[derive(Debug, Default)]
pub struct PageMap {
    regions: BTreeMap<u32, Region>,
    /// The loaded modules' executable objects: start page to end (bytes,
    /// exclusive), for `DosQueryMem`'s `PAG_EXECUTE`.
    executable: BTreeMap<u32, u32>,
}

impl PageMap {
    /// The region holding `addr`.
    fn find(&self, addr: u32) -> Option<(u32, &Region)> {
        let (&base, region) = self.regions.range(..=addr).next_back()?;
        (u64::from(addr) < region.end(base)).then_some((base, region))
    }

    fn find_mut(&mut self, addr: u32) -> Option<(u32, &mut Region)> {
        let (&base, region) = self.regions.range_mut(..=addr).next_back()?;
        (u64::from(addr) < region.end(base)).then_some((base, region))
    }

    /// Record a module's executable object of `bytes` at `base`.
    pub fn mark_executable(&mut self, base: u32, bytes: u32) {
        let start = base & !(PAGE_SIZE - 1);
        let end = base.saturating_add(bytes);
        if end > start {
            self.executable.insert(start, end);
        }
    }

    /// Forget the executable object at `base` (its module was freed).
    pub fn forget_executable(&mut self, base: u32) {
        self.executable.remove(&(base & !(PAGE_SIZE - 1)));
    }

    /// Whether the page at `page` holds code of a loaded module.
    pub fn executable(&self, page: u32) -> bool {
        self.executable.range(..=page).next_back().is_some_and(|(_, &end)| page < end)
    }

    /// Whether `addr` lies in a tracked allocation.
    pub fn contains(&self, addr: u32) -> bool {
        self.find(addr).is_some()
    }

    /// Record an allocation of `size` bytes (whole pages) at `base` with
    /// `DosAllocMem`'s `flag`; every page's entry.
    pub fn allocate(&mut self, base: u32, size: u32, flag: u32) -> Vec<Change> {
        let count = size.div_ceil(PAGE_SIZE) as usize;
        let state = flag & STATE_BITS;
        self.regions.insert(base, Region { pages: vec![state; count], default: flag & FPERM, stack: false, exhausted: false });
        let access = access_of(state);
        (0..count as u32).map(|i| (base + i * PAGE_SIZE, access)).collect()
    }

    /// Record a thread's sparse stack of `size` bytes at `base` (ADR-0114,
    /// `STACK_SPARSE`): its top page committed, a guard page below it, the
    /// rest reserved. A stack of fewer than [`MIN_SPARSE_STACK_PAGES`]
    /// pages is not recorded (committed whole). Every page's entry.
    pub fn allocate_stack(&mut self, base: u32, size: u32) -> Vec<Change> {
        let count = (size / PAGE_SIZE) as usize;
        if count < MIN_SPARSE_STACK_PAGES as usize {
            return Vec::new();
        }
        let rw = PAG_READ | PAG_WRITE;
        let mut pages = vec![0; count];
        pages[count - 1] = PAG_COMMIT | rw;
        pages[count - 2] = PAG_COMMIT | rw | PAG_GUARD;
        self.regions.insert(base, Region { pages, default: rw, stack: true, exhausted: false });
        (0..count as u32).map(|i| (base + i * PAGE_SIZE, if i as usize == count - 1 { PageAccess::ReadWrite } else { PageAccess::None })).collect()
    }

    /// Forget the allocation at `base`: its pages are not present any
    /// more, as free memory is not (phase 4). None: no allocation starts
    /// there.
    pub fn free(&mut self, base: u32) -> Option<Vec<Change>> {
        let region = self.regions.remove(&base)?;
        Some(region.pages.iter().enumerate()
            .filter(|&(_, &s)| access_of(s) != PageAccess::None)
            .map(|(i, _)| (base + i as u32 * PAGE_SIZE, PageAccess::None))
            .collect())
    }

    /// `DosSetMem` on `cb` bytes at `addr` with `flag`. None: `addr` is in
    /// no tracked allocation (the caller decides). Err: an OS/2 error.
    pub fn set(&mut self, addr: u32, cb: u32, flag: u32) -> Option<Result<SetOutcome, u32>> {
        let (base, region) = self.find_mut(addr)?;
        Some(set_pages(base, region, addr, cb, flag))
    }

    /// `DosQueryMem` at `addr` for at most `cb` bytes: the bytes from the
    /// page holding `addr` whose pages have the same state, and their
    /// flags. None: `addr` is in no tracked allocation.
    pub fn query(&self, addr: u32, cb: u32) -> Option<(u32, u32)> {
        let (base, region) = self.find(addr)?;
        let first = ((addr - base) / PAGE_SIZE) as usize;
        let state = region.pages[first];
        let limit = cb.div_ceil(PAGE_SIZE).max(1) as usize;
        let run = region.pages[first..].iter().take(limit).take_while(|&&s| s == state).count() as u32;
        // An uncommitted page reports the access it was allocated with.
        let mut flags = if state & PAG_COMMIT != 0 { state } else { region.default | (state & PAG_GUARD) };
        if first == 0 {
            flags |= PAG_BASE;
        }
        Some((run * PAGE_SIZE, flags))
    }

    /// A page fault at `addr` (a write when `write`, an instruction fetch
    /// when `fetch`): what it means for a tracked page, and the entries
    /// that change (a guard taken off, a stack grown); None for a page in
    /// no tracked allocation.
    pub fn fault(&mut self, addr: u32, write: bool, fetch: bool) -> Option<(FaultKind, Vec<Change>)> {
        let (base, region) = self.find_mut(addr)?;
        let i = ((addr - base) / PAGE_SIZE) as usize;
        let state = region.pages[i];
        let page = base + i as u32 * PAGE_SIZE;
        let a = access_of(state);
        // A fetch from a page without PAG_EXECUTE is never a stale entry.
        let allows = a.present() && (!write || a.writable()) && (!fetch || a.executable());
        // The entry is rewritten from the state: a stale TLB entry, or an
        // entry that disagrees with the state, cannot fault for ever.
        if allows {
            return Some((FaultKind::Retry, vec![(page, access_of(state))]));
        }
        // A fetch from a committed page without PAG_EXECUTE (ADR-0131): no
        // guard to take off, no stack to grow.
        if fetch && a.present() {
            return Some((FaultKind::Violation, vec![]));
        }
        if region.stack {
            return Some(grow_stack(base, region, i));
        }
        if state & PAG_GUARD != 0 && state & PAG_COMMIT != 0 {
            region.pages[i] = state & !PAG_GUARD;
            return Some((FaultKind::Guard, vec![(page, access_of(region.pages[i]))]));
        }
        Some((FaultKind::Violation, Vec::new()))
    }
}

/// A touch of page `i` of a sparse stack below its committed part: commit
/// every page from there up to the committed part, and make the page below
/// a guard page (Warpine commits any page touched, not only through the
/// guard page, since a frame may be larger than a page and Warpine writes
/// handler frames below the stack pointer itself). The bottom pages raise
/// `XCPT_UNABLE_TO_GROW_STACK` the first time.
fn grow_stack(base: u32, region: &mut Region, i: usize) -> (FaultKind, Vec<Change>) {
    let rw = PAG_COMMIT | PAG_READ | PAG_WRITE;
    let mut changes = Vec::new();
    let mut j = i;
    while j < region.pages.len() && region.pages[j] != rw {
        region.pages[j] = rw;
        changes.push((base + j as u32 * PAGE_SIZE, PageAccess::ReadWrite));
        j += 1;
    }
    if i > 0 && region.pages[i - 1] & PAG_COMMIT == 0 {
        region.pages[i - 1] = rw | PAG_GUARD;
    }
    let kind = if i < STACK_RESERVE_PAGES && !region.exhausted {
        region.exhausted = true;
        FaultKind::StackExhausted
    } else {
        FaultKind::Retry
    };
    (kind, changes)
}

/// `DosSetMem` on the pages of one region (see [`PageMap::set`]).
fn set_pages(base: u32, region: &mut Region, addr: u32, cb: u32, flag: u32) -> Result<SetOutcome, u32> {
    if cb == 0 || flag & !SET_FLAGS != 0 || flag & PAG_COMMIT != 0 && flag & PAG_DECOMMIT != 0 {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let first = ((addr - base) / PAGE_SIZE) as usize;
    let last = addr.checked_add(cb - 1).map(|end| ((end.wrapping_sub(base)) / PAGE_SIZE) as usize);
    let Some(last) = last.filter(|&l| u64::from(addr) + u64::from(cb) <= region.end(base) && l < region.pages.len()) else {
        return Err(ERROR_INVALID_ADDRESS);
    };
    let pages = first..=last;
    let committed = |s: u32| s & PAG_COMMIT != 0;
    let access = if flag & PAG_DEFAULT != 0 { region.default } else { flag & FPERM };
    let guard = flag & PAG_GUARD;
    let new_state = if flag & PAG_DECOMMIT != 0 {
        if flag & !PAG_DECOMMIT != 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        if !region.pages[pages.clone()].iter().all(|&s| committed(s)) {
            return Err(ERROR_ACCESS_DENIED);
        }
        0
    } else {
        if access == 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        // Committing committed pages, or changing uncommitted ones, is refused.
        let must_be_committed = flag & PAG_COMMIT == 0;
        if region.pages[pages.clone()].iter().any(|&s| committed(s) != must_be_committed) {
            return Err(ERROR_ACCESS_DENIED);
        }
        PAG_COMMIT | access | guard
    };
    let mut out = SetOutcome::default();
    for i in pages {
        let page = base + i as u32 * PAGE_SIZE;
        let (old, new) = (access_of(region.pages[i]), access_of(new_state));
        if !committed(region.pages[i]) && committed(new_state) {
            out.committed.push(page);
        }
        region.pages[i] = new_state;
        if old != new {
            out.downgrade |= loses_access(old, new);
            out.changes.push((page, new));
        }
    }
    Ok(out)
}

/// TLB shootdowns (ADR-0114 addendum): when pages lose access, the call
/// that took it away returns only once no vCPU can still reach them
/// through a stale TLB entry. Each vCPU says whether it is in guest code
/// and the epoch its TLB was last flushed for; [`TlbSync::shoot_down`]
/// counts the epoch up, and its caller kicks every vCPU and waits.
#[derive(Debug, Default)]
pub struct TlbSync {
    epoch: AtomicU64,
    vcpus: Mutex<Vec<Weak<VcpuTlb>>>,
}

/// One vCPU's side of [`TlbSync`].
#[derive(Debug, Default)]
pub struct VcpuTlb {
    in_guest: AtomicBool,
    seen: AtomicU64,
}

/// How long a shootdown waits for the vCPUs before it gives up with a
/// warning: a kicked vCPU leaves guest code within microseconds.
pub const SHOOTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

impl TlbSync {
    /// A new vCPU, clean for the current epoch.
    pub fn register(&self) -> Arc<VcpuTlb> {
        let vcpu = Arc::new(VcpuTlb { in_guest: AtomicBool::new(false), seen: AtomicU64::new(self.current()) });
        let mut vcpus = self.vcpus.lock_or_recover();
        vcpus.retain(|v| v.strong_count() > 0);
        vcpus.push(Arc::downgrade(&vcpu));
        vcpu
    }

    /// The current epoch.
    pub fn current(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// `vcpu` is about to enter guest code: the epoch its TLB must be
    /// clean for. Marked in guest code first, so that a shootdown counting
    /// the epoch up after this either is seen here or waits for the vCPU.
    pub fn enter(&self, vcpu: &VcpuTlb) -> u64 {
        vcpu.in_guest.store(true, Ordering::SeqCst);
        self.current()
    }

    /// `vcpu`'s TLB is clean for `epoch` (flushed, or nothing changed).
    pub fn clean(vcpu: &VcpuTlb, epoch: u64) {
        vcpu.seen.store(epoch, Ordering::SeqCst);
    }

    /// `vcpu` left guest code.
    pub fn leave(vcpu: &VcpuTlb) {
        vcpu.in_guest.store(false, Ordering::SeqCst);
    }

    /// Count the epoch up: the one every vCPU must reach.
    pub fn shoot_down(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Wait until every vCPU is out of guest code or clean for `epoch`;
    /// false when `timeout` passed first.
    pub fn wait(&self, epoch: u64, timeout: Duration) -> bool {
        let vcpus: Vec<Arc<VcpuTlb>> = self.vcpus.lock_or_recover().iter().filter_map(Weak::upgrade).collect();
        let start = Instant::now();
        let mut spins = 0u32;
        loop {
            let reached = |v: &Arc<VcpuTlb>| !v.in_guest.load(Ordering::SeqCst) || v.seen.load(Ordering::SeqCst) >= epoch;
            if vcpus.iter().all(reached) {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            spins += 1;
            if spins < 64 { std::thread::yield_now() } else { std::thread::sleep(Duration::from_micros(50)) }
        }
    }
}

impl super::SharedState {
    /// Take pages out of every vCPU's TLB (ADR-0114 addendum): count the
    /// epoch up, kick every vCPU out of guest code and wait until each has
    /// flushed or left it.
    pub(crate) fn shoot_down_tlbs(&self) {
        let epoch = self.tlb.shoot_down();
        self.threads.table.lock_or_recover().kick_all_but(0);
        if !self.tlb.wait(epoch, SHOOTDOWN_TIMEOUT) {
            log::warn!("TLB shootdown: a vCPU did not leave guest code within {SHOOTDOWN_TIMEOUT:?}");
        }
    }
}

impl super::Loader {
    /// Write `changes` into the process tables, which are written first if
    /// no thread has run yet (a program's objects are protected before
    /// its first thread exists).
    fn apply_page_changes(&self, changes: &[Change]) {
        if changes.is_empty() {
            return;
        }
        self.ensure_process_tables();
        // The allocator's changes first, so that these come after them.
        self.apply_pending_allocations();
        for &(addr, access) in changes {
            if let Some(at) = process_pte_at(addr) {
                self.system_write::<u64>(at, process_pte(addr, access));
            }
        }
    }

    /// Bring the allocator's pages into the process tables: allocated
    /// pages present and writable, freed pages not present (ADR-0114,
    /// phase 4). Pages of `DosAllocMem` regions keep their own states.
    /// A process without page tables (a 16-bit program at ring 0) only
    /// drops the record.
    pub(crate) fn sync_allocations(&self) {
        if !self.shared.alloc_pending.load(Ordering::Acquire) {
            // Nothing new; but a batch another thread took (which clears
            // the flag) may still be being written: wait for it, so the
            // tables hold every change made before this call.
            drop(self.shared.pte_sync.lock_or_recover());
            return;
        }
        if !self.shared.process_tables_ready.load(Ordering::Acquire) {
            self.shared.mem_mgr.lock_or_recover().take_pending();
            return;
        }
        self.apply_pending_allocations();
    }

    /// The allocator's pending changes into the process tables. Frees do
    /// not flush other vCPUs' TLBs (Warpine frees often, a callback's
    /// scratch memory): a stale entry only lets another thread reach
    /// memory just freed; the freeing thread flushes its own
    /// (`managers::take_freed_here`).
    fn apply_pending_allocations(&self) {
        // Taken and written as one: another thread's batch must not be
        // written between them (`SharedState::pte_sync`).
        let _ordered = self.shared.pte_sync.lock_or_recover();
        let pending = self.shared.mem_mgr.lock_or_recover().take_pending();
        if pending.is_empty() {
            return;
        }
        let writes: Vec<(u32, u64)> = {
            let vmem = self.shared.vmem.lock_or_recover();
            pending.iter()
                .flat_map(|&(addr, size, allocated)| (0..size / PAGE_SIZE).map(move |i| (addr + i * PAGE_SIZE, allocated)))
                .filter(|&(page, _)| !vmem.contains(page))
                .filter_map(|(page, allocated)| {
                    // A module's code object is executable (ADR-0131).
                    let access = if allocated { PageAccess::ReadWrite.with_exec(vmem.executable(page)) } else { PageAccess::None };
                    process_pte_at(page).map(|at| (at, process_pte(page, access)))
                })
                .collect()
        };
        for (at, entry) in writes {
            self.system_write::<u64>(at, entry);
        }
    }

    /// Whether `addr` is an allocated page whose process-table entry is
    /// present now (a fault there came before this vCPU saw the
    /// allocation: run the instruction again).
    pub(crate) fn allocated_and_present(&self, addr: u32) -> bool {
        self.sync_allocations();
        let allocated = self.shared.mem_mgr.lock_or_recover().contains(addr);
        allocated && process_pte_at(addr)
            .and_then(|at| self.guest_read::<u64>(at))
            .is_some_and(|e| e & 1 != 0)
    }

    /// Make every vCPU drop the TLB entries of pages that lost access:
    /// count the epoch up and kick them; each reloads CR3 before it runs
    /// guest code again (`vcpu.rs`).
    pub(crate) fn flush_tlbs(&self) {
        self.shared.shoot_down_tlbs();
    }

    /// DosAllocMem (ordinal 299): reserve `cb` bytes, committed with
    /// `PAG_COMMIT`, with the access of `flag`; `*ppb` gets the address.
    pub fn dos_alloc_mem(&self, ppb: u32, cb: u32, flag: u32) -> u32 {
        debug!("DosAllocMem(ppb=0x{ppb:08X}, cb=0x{cb:08X}, flag=0x{flag:X})");
        if cb == 0 || flag & !ALLOC_FLAGS != 0 || flag & FPERM == 0
            || flag & PAG_GUARD != 0 && flag & PAG_COMMIT == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        // Committed memory is zero (a reused block is cleared).
        let Some(addr) = self.alloc_zeroed(cb) else { return ERROR_NOT_ENOUGH_MEMORY };
        if self.guest_write::<u32>(ppb, addr).is_none() {
            self.shared.mem_mgr.lock_or_recover().free(addr);
            return ERROR_INVALID_PARAMETER;
        }
        let size = cb.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let changes = self.shared.vmem.lock_or_recover().allocate(addr, size, flag);
        self.apply_page_changes(&changes);
        // A page of a block freed before may still be in a TLB, writable.
        if changes.iter().any(|c| c.1 != PageAccess::ReadWrite) {
            self.flush_tlbs();
        }
        debug!("  -> allocated at 0x{addr:08X}");
        NO_ERROR
    }

    /// Make the read-only objects of a loaded module read-only (ADR-0114)
    /// and its executable objects executable (ADR-0131; every other page
    /// is not): `objects` are `(base, bytes, LX flags)`, after the fixups
    /// are in. Its executable objects report `PAG_EXECUTE` (`DosQueryMem`).
    pub(crate) fn protect_objects(&self, objects: &[(u32, u32, u32)]) {
        let mut code = std::collections::BTreeSet::new();
        {
            let mut vmem = self.shared.vmem.lock_or_recover();
            for &(base, bytes, _) in objects.iter().filter(|o| o.2 & LX_OBJECT_EXEC != 0) {
                vmem.mark_executable(base, bytes);
                let first = base & !(PAGE_SIZE - 1);
                let end = (u64::from(base) + u64::from(bytes)).div_ceil(u64::from(PAGE_SIZE)) * u64::from(PAGE_SIZE);
                code.extend((u64::from(first)..end).step_by(PAGE_SIZE as usize).map(|p| p as u32));
            }
        }
        let read_only: std::collections::BTreeSet<u32> = read_only_pages(objects).into_iter().collect();
        let changes: Vec<Change> = read_only.union(&code).copied()
            .filter(|&p| process_pte_at(p).is_some())
            .map(|p| {
                let access = if read_only.contains(&p) { PageAccess::Read } else { PageAccess::ReadWrite };
                (p, access.with_exec(code.contains(&p)))
            }).collect();
        self.apply_page_changes(&changes);
        // A module loaded while threads run may reuse pages a TLB holds
        // writable.
        if !changes.is_empty() {
            self.flush_tlbs();
        }
    }

    /// Free a module's objects (from the allocator): their pages are
    /// present and writable again before the memory can be handed out.
    pub(crate) fn free_module_objects(&self, bases: impl IntoIterator<Item = u32>) {
        for base in bases {
            self.shared.vmem.lock_or_recover().forget_executable(base);
            let size = self.shared.mem_mgr.lock_or_recover().block_size(base);
            if let Some(size) = size {
                let changes: Vec<Change> = (0..size / PAGE_SIZE).map(|i| (base + i * PAGE_SIZE, PageAccess::ReadWrite)).collect();
                self.apply_page_changes(&changes);
            }
            self.shared.mem_mgr.lock_or_recover().free(base);
        }
    }

    /// Make the thread stack of `size` bytes at `base` sparse (ADR-0114).
    pub(crate) fn sparse_thread_stack(&self, base: u32, size: u32) {
        let size = size.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let changes = self.shared.vmem.lock_or_recover().allocate_stack(base, size);
        self.apply_page_changes(&changes);
        // The pages may have been present and writable before (a stack of
        // an ended thread, say): a TLB may still hold them so.
        if changes.iter().any(|c| c.1 != PageAccess::ReadWrite) {
            self.flush_tlbs();
        }
    }

    /// Thread 1's stack (ADR-0114): its bounds in the TIB, and its pages
    /// sparse as other threads' stacks are (`allocate_stack`). A stack too
    /// small to be sparse is left committed.
    pub(crate) fn sparse_main_stack(&self, stack: MainStack) {
        self.guest_write::<u32>(TIB_BASE + TIB_PSTACK_OFFSET, stack.bottom);
        self.guest_write::<u32>(TIB_BASE + TIB_PSTACKLIMIT_OFFSET, stack.top);
        let (base, bytes) = stack.sparse_pages();
        if bytes != 0 {
            self.sparse_thread_stack(base, bytes);
        }
    }

    /// The page states `DosFreeMem` gives up with the block at `pb`.
    pub(crate) fn free_page_states(&self, pb: u32) {
        let changes = self.shared.vmem.lock_or_recover().free(pb);
        if let Some(changes) = changes {
            self.apply_page_changes(&changes);
        }
    }

    /// DosSetMem (ordinal 305): commit, decommit, or change the access of
    /// `cb` bytes at `pb`. Memory `DosAllocMem` did not allocate (shared
    /// memory, the program's objects) is left as it is.
    pub fn dos_set_mem(&self, pb: u32, cb: u32, flag: u32) -> u32 {
        debug!("  DosSetMem(pb=0x{pb:08X}, cb=0x{cb:X}, flag=0x{flag:X})");
        let outcome = self.shared.vmem.lock_or_recover().set(pb, cb, flag);
        let outcome = match outcome {
            None => return NO_ERROR,
            Some(Err(e)) => return e,
            Some(Ok(o)) => o,
        };
        for &page in &outcome.committed {
            self.guest_write_bytes(page, &[0u8; PAGE_SIZE as usize]);
        }
        self.apply_page_changes(&outcome.changes);
        if outcome.downgrade {
            self.flush_tlbs();
        }
        NO_ERROR
    }

    /// DosQueryMem (ordinal 306): the size of the run of pages at `pb`
    /// (at most `*pcb` bytes) with the same state, into `*pcb`, and their
    /// flags into `*pflag`.
    pub fn dos_query_mem(&self, pb: u32, pcb: u32, pflag: u32) -> u32 {
        debug!("  DosQueryMem(pb=0x{pb:08X})");
        let Some(cb) = self.guest_read::<u32>(pcb) else { return ERROR_INVALID_PARAMETER };
        if cb == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let tracked = self.shared.vmem.lock_or_recover().query(pb, cb);
        let (size, flags) = tracked.unwrap_or_else(|| self.query_untracked(pb, cb));
        debug!("  -> {size:#x} bytes, flags {flags:#x}");
        if self.guest_write::<u32>(pcb, size).is_none()
            || self.guest_write::<u32>(pflag, flags).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    /// Whether an exception no handler took goes on by default: a guard
    /// page violation, whose guard is gone (Control Program Reference).
    pub(crate) fn continues_by_default(&self, exc_report: u32) -> bool {
        self.guest_read::<u32>(exc_report.wrapping_add(ERR_NUM)) == Some(XCPT_GUARD_PAGE_VIOLATION)
    }

    /// `DosQueryMem` on memory `DosAllocMem` did not allocate: what the
    /// process tables say (after the allocator's pending changes), for the
    /// run of pages with the same entry: `PAG_FREE` for a page not present,
    /// read-only or writable committed memory otherwise (`PAG_EXECUTE` too
    /// in a module's executable object), `PAG_BASE` on the
    /// first page of an allocator block. Memory the process tables do not
    /// map (shared memory, Warpine's own areas) reads as committed and
    /// writable, a page at a time, `PAG_SHARED` in the shared arena.
    fn query_untracked(&self, pb: u32, cb: u32) -> (u32, u32) {
        if let Some(answer) = self.arena_query(pb, cb) {
            return answer;
        }
        self.sync_allocations();
        let first = pb & !(PAGE_SIZE - 1);
        let limit = cb.div_ceil(PAGE_SIZE).max(1);
        // A copy: the allocator's lock is held below, and the page states'
        // is not taken under it.
        let code = PageMap { executable: self.shared.vmem.lock_or_recover().executable.clone(), ..Default::default() };
        let flags_at = |page: u32| -> Option<u32> {
            if !self.shared.process_tables_ready.load(Ordering::Acquire) {
                return None;
            }
            let entry = self.guest_read::<u64>(process_pte_at(page)?)?;
            const PRESENT: u64 = 1 << 0;
            const WRITABLE: u64 = 1 << 1;
            let execute = if code.executable(page) { PAG_EXECUTE } else { 0 };
            Some(match (entry & PRESENT != 0, entry & WRITABLE != 0) {
                (false, _) => PAG_FREE,
                (true, false) => PAG_COMMIT | PAG_READ | execute,
                (true, true) => PAG_COMMIT | PAG_READ | PAG_WRITE | execute,
            })
        };
        let Some(state) = flags_at(first) else {
            let shared = if super::shared_arena::in_arena(pb) { PAG_SHARED } else { 0 };
            return (PAGE_SIZE.min(limit * PAGE_SIZE), PAG_COMMIT | PAG_READ | PAG_WRITE | shared);
        };
        let mem = self.shared.mem_mgr.lock_or_recover();
        // A run ends at the limit, at another state, or where the next
        // allocator block starts.
        let run = (0..limit)
            .take_while(|&i| {
                let page = first.wrapping_add(i * PAGE_SIZE);
                (i == 0 || mem.block_size(page).is_none()) && flags_at(page) == Some(state)
            })
            .count() as u32;
        let base = if state != PAG_FREE && mem.block_size(first).is_some() { PAG_BASE } else { 0 };
        (run * PAGE_SIZE, state | base)
    }

    /// A page fault at `addr` from guest code (a write when `write`): what
    /// it means, with the guard taken off a guard page. None: the page is
    /// in no tracked allocation.
    pub(crate) fn page_fault_kind(&self, addr: u32, write: bool, fetch: bool) -> Option<FaultKind> {
        if super::shared_arena::in_arena(addr) {
            return self.arena_fault(addr, write, fetch);
        }
        let (kind, changes) = self.shared.vmem.lock_or_recover().fault(addr, write, fetch)?;
        self.apply_page_changes(&changes);
        Some(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x0300_0000;
    const RW: u32 = PAG_COMMIT | PAG_READ | PAG_WRITE;

    /// A shootdown waits for a vCPU in guest code until it is clean for
    /// the new epoch or has left guest code; vCPUs outside guest code and
    /// gone ones never hold it up (ADR-0114 addendum).
    #[test]
    fn a_tlb_shootdown_waits_for_every_vcpu_in_guest_code() {
        let short = Duration::from_millis(20);
        let sync = TlbSync::default();
        let (a, b) = (sync.register(), sync.register());
        let gone = sync.register();
        let start = sync.enter(&gone);
        TlbSync::clean(&gone, start);
        drop(gone);
        assert!(sync.wait(sync.shoot_down(), short), "nobody in guest code, the gone vCPU uncounted");
        let old = sync.enter(&a);
        TlbSync::clean(&a, old);
        let epoch = sync.shoot_down();
        assert!(!sync.wait(epoch, short), "a runs with a TLB older than the shootdown");
        TlbSync::leave(&a);
        assert!(sync.wait(epoch, short), "a left guest code");
        assert_eq!(sync.enter(&b), epoch, "b enters after it: it sees the new epoch");
        TlbSync::clean(&b, epoch);
        assert!(sync.wait(epoch, short), "b is clean for it");
        let epoch = sync.shoot_down();
        let waiter = std::thread::scope(|s| {
            let w = s.spawn(|| sync.wait(epoch, SHOOTDOWN_TIMEOUT));
            std::thread::sleep(short);
            TlbSync::clean(&b, sync.current());
            w.join().unwrap()
        });
        assert!(waiter, "the waiter goes on once b flushed");
    }

    /// Thread 1's stack is the `STACKSIZE` bytes below the entry ESP, in
    /// its object; whole pages above a page shared with data are sparse.
    #[test]
    fn the_main_stack_lies_below_the_entry_esp() {
        // Open Watcom: data and the stack in one object, ESP at its end.
        let watcom = MainStack::of(0x20000, 0x101F0, 0x301F0, 0x10000).expect("a stack");
        assert_eq!(watcom, MainStack { bottom: 0x201F0, top: 0x301F0 });
        assert_eq!(watcom.sparse_pages(), (0x21000, 0x10000), "the page shared with data stays committed");
        // lx-link: a stack object of its own.
        let own = MainStack::of(0x30000, 0x10000, 0x40000, 0x10000).expect("a stack");
        assert_eq!(own.sparse_pages(), (0x30000, 0x10000));
        assert_eq!(MainStack::of(0x30000, 0x8000, 0x38000, 0x10000).map(|s| s.bottom), Some(0x30000), "not below its object");
        assert_eq!(MainStack::of(0x30000, 0x10000, 0x40000, 0), None, "no STACKSIZE");
        assert_eq!(MainStack::of(0x30000, 0x10000, 0x40004, 0x1000), None, "ESP past the object");
        assert_eq!(MainStack::of(0x30000, 0x10000, 0x30000, 0x1000), None, "ESP at the object's base");
        let tiny = MainStack::of(0x30000, 0x10000, 0x30800, 0x100).expect("a stack");
        assert_eq!(tiny.sparse_pages(), (0x31000, 0), "no whole page: nothing sparse");
    }

    /// Executable objects are known by page until their module is freed.
    #[test]
    fn executable_objects_are_known_by_page() {
        let mut map = PageMap::default();
        map.mark_executable(0x10000, 0x1801);
        assert!(map.executable(0x10000) && map.executable(0x11000));
        assert!(!map.executable(0x12000), "past the object");
        assert!(!map.executable(0xF000), "before it");
        map.mark_executable(0x20000, 0);
        assert!(!map.executable(0x20000), "an empty object");
        map.forget_executable(0x10000);
        assert!(!map.executable(0x10000));
    }

    #[test]
    fn a_committed_writable_allocation_is_present_and_writable() {
        let mut map = PageMap::default();
        assert_eq!(map.allocate(BASE, 3 * PAGE_SIZE, RW).len(), 3, "every page's entry, present and writable");
        assert!(map.contains(BASE + 2 * PAGE_SIZE + 5));
        assert!(!map.contains(BASE + 3 * PAGE_SIZE));
        assert_eq!(map.query(BASE, 0x10000), Some((3 * PAGE_SIZE, RW | PAG_BASE)));
        assert_eq!(map.query(BASE + PAGE_SIZE, 1), Some((PAGE_SIZE, RW)), "not the base; one page asked for");
    }

    #[test]
    fn reserved_read_only_and_guard_pages_get_their_entries() {
        let mut map = PageMap::default();
        let reserved = map.allocate(BASE, 2 * PAGE_SIZE, PAG_READ | PAG_WRITE);
        assert_eq!(reserved, vec![(BASE, PageAccess::None), (BASE + PAGE_SIZE, PageAccess::None)]);
        assert_eq!(map.query(BASE, PAGE_SIZE * 2), Some((2 * PAGE_SIZE, PAG_READ | PAG_WRITE | PAG_BASE)), "uncommitted: the allocation's access");
        let read = map.allocate(BASE + 0x10000, PAGE_SIZE, PAG_COMMIT | PAG_READ);
        assert_eq!(read, vec![(BASE + 0x10000, PageAccess::Read)]);
        let guard = map.allocate(BASE + 0x20000, PAGE_SIZE, RW | PAG_GUARD);
        assert_eq!(guard, vec![(BASE + 0x20000, PageAccess::None)]);
        assert!(map.free(BASE).unwrap().is_empty(), "reserved pages: not present already");
        assert_eq!(map.free(BASE + 0x10000).unwrap(), vec![(BASE + 0x10000, PageAccess::None)], "freed: not present");
        assert_eq!(map.free(BASE), None);
        assert!(!map.contains(BASE));
    }

    #[test]
    fn set_mem_commits_decommits_and_changes_access() {
        let mut map = PageMap::default();
        map.allocate(BASE, 4 * PAGE_SIZE, PAG_READ | PAG_WRITE);
        let o = map.set(BASE + PAGE_SIZE, 2 * PAGE_SIZE, PAG_COMMIT | PAG_DEFAULT).unwrap().unwrap();
        assert_eq!(o.committed, vec![BASE + PAGE_SIZE, BASE + 2 * PAGE_SIZE]);
        assert_eq!(o.changes, vec![(BASE + PAGE_SIZE, PageAccess::ReadWrite), (BASE + 2 * PAGE_SIZE, PageAccess::ReadWrite)]);
        assert!(!o.downgrade);
        assert_eq!(map.query(BASE, 0x10000), Some((PAGE_SIZE, PAG_READ | PAG_WRITE | PAG_BASE)), "page 0 still reserved");
        assert_eq!(map.query(BASE + PAGE_SIZE, 0x10000), Some((2 * PAGE_SIZE, RW)));
        let o = map.set(BASE + PAGE_SIZE, 1, PAG_READ).unwrap().unwrap();
        assert_eq!((o.changes, o.downgrade), (vec![(BASE + PAGE_SIZE, PageAccess::Read)], true), "read-only: a downgrade");
        let o = map.set(BASE + 2 * PAGE_SIZE, PAGE_SIZE, PAG_DECOMMIT).unwrap().unwrap();
        assert_eq!((o.changes, o.downgrade), (vec![(BASE + 2 * PAGE_SIZE, PageAccess::None)], true));
        assert_eq!(map.query(BASE + 2 * PAGE_SIZE, PAGE_SIZE), Some((PAGE_SIZE, PAG_READ | PAG_WRITE)));
    }

    #[test]
    fn set_mem_refuses_what_os2_refuses() {
        let mut map = PageMap::default();
        map.allocate(BASE, 2 * PAGE_SIZE, RW);
        assert_eq!(map.set(BASE + 3 * PAGE_SIZE, 1, RW), None, "outside every allocation");
        assert_eq!(map.set(BASE, PAGE_SIZE, PAG_COMMIT | PAG_READ), Some(Err(ERROR_ACCESS_DENIED)), "committed already");
        assert_eq!(map.set(BASE, 3 * PAGE_SIZE, PAG_READ), Some(Err(ERROR_INVALID_ADDRESS)), "past the allocation");
        assert_eq!(map.set(BASE, 0, PAG_READ), Some(Err(ERROR_INVALID_PARAMETER)), "no bytes");
        assert_eq!(map.set(BASE, 1, PAG_COMMIT | PAG_DECOMMIT), Some(Err(ERROR_INVALID_PARAMETER)));
        assert_eq!(map.set(BASE, 1, PAG_DECOMMIT | PAG_READ), Some(Err(ERROR_INVALID_PARAMETER)));
        assert_eq!(map.set(BASE, 1, 0), Some(Err(ERROR_INVALID_PARAMETER)), "no access");
        assert_eq!(map.set(BASE, 1, 0x8000_0000), Some(Err(ERROR_INVALID_PARAMETER)), "an unknown flag");
        map.set(BASE, PAGE_SIZE, PAG_DECOMMIT).unwrap().unwrap();
        assert_eq!(map.set(BASE, PAGE_SIZE, PAG_DECOMMIT), Some(Err(ERROR_ACCESS_DENIED)), "not committed");
        assert_eq!(map.set(BASE, PAGE_SIZE, PAG_READ), Some(Err(ERROR_ACCESS_DENIED)), "access of an uncommitted page");
    }

    /// Pages of objects without the write flag are read-only, but for one a
    /// writable object shares; byte sizes round up to whole pages.
    #[test]
    fn read_only_objects_lose_only_their_own_pages() {
        const CODE: u32 = 0x0001 | 0x0004;
        const DATA: u32 = 0x0001 | LX_OBJECT_WRITE;
        assert_eq!(read_only_pages(&[(0x10000, 0x2100, CODE), (0x20000, 0x1000, DATA)]),
                   vec![0x10000, 0x11000, 0x12000], "a partial last page counts");
        assert_eq!(read_only_pages(&[(0x10000, 0x1800, CODE), (0x11000, 0x1000, DATA)]),
                   vec![0x10000], "the page shared with data stays writable");
        assert!(read_only_pages(&[(0x10000, 0x1000, DATA)]).is_empty());
        assert!(read_only_pages(&[(0x10000, 0, CODE)]).is_empty(), "no bytes");
        assert_eq!(read_only_pages(&[(u32::MAX - 0xFFF, 0x2000, CODE)]), vec![u32::MAX - 0xFFF], "no overflow");
    }

    /// DosQueryMem on memory DosAllocMem did not allocate answers from the
    /// process tables: an allocator block committed and writable, its first
    /// page the base, the run ending at the next block; free pages
    /// PAG_FREE; a read-only page read-only; a code object's page
    /// PAG_EXECUTE until its module is freed.
    #[test]
    fn query_mem_answers_untracked_memory_from_the_page_tables() {
        let l = super::super::Loader::new_mock();
        l.ensure_process_tables();
        let (pcb, pflag) = (0x3000, 0x3004);
        let query = |pb: u32, cb: u32| {
            l.guest_write::<u32>(pcb, cb).unwrap();
            assert_eq!(l.dos_query_mem(pb, pcb, pflag), NO_ERROR);
            (l.guest_read::<u32>(pcb).unwrap(), l.guest_read::<u32>(pflag).unwrap())
        };
        let a = l.shared.mem_mgr.lock().unwrap().alloc(2 * PAGE_SIZE).unwrap();
        let b = l.shared.mem_mgr.lock().unwrap().alloc(PAGE_SIZE).unwrap();
        assert_eq!(b, a + 2 * PAGE_SIZE, "the next block right after");
        assert_eq!(query(a, 0x10000), (2 * PAGE_SIZE, RW | PAG_BASE), "the run ends where the next block starts");
        assert_eq!(query(a + PAGE_SIZE, 0x10000), (PAGE_SIZE, RW), "not the base");
        assert_eq!(query(b + PAGE_SIZE, 2 * PAGE_SIZE), (2 * PAGE_SIZE, PAG_FREE), "free pages");
        l.protect_objects(&[(a, PAGE_SIZE, LX_OBJECT_EXEC | 0x0001)]);
        assert_eq!(query(a, 0x10000), (PAGE_SIZE, PAG_COMMIT | PAG_READ | PAG_EXECUTE | PAG_BASE), "a code object page");
        l.protect_objects(&[(a + PAGE_SIZE, PAGE_SIZE, 0x0001)]);
        assert_eq!(query(a + PAGE_SIZE, PAGE_SIZE), (PAGE_SIZE, PAG_COMMIT | PAG_READ), "a read-only data page: no PAG_EXECUTE");
        l.free_module_objects([a]);
        assert_eq!(query(a, PAGE_SIZE), (PAGE_SIZE, PAG_FREE), "freed: no PAG_EXECUTE left");
        // Without a session the arena holds no block (ADR-0049 addendum).
        assert_eq!(query(SHARED_ARENA_BASE, PAGE_SIZE), (PAGE_SIZE, PAG_FREE));
    }

    /// A sparse stack: its top page committed, a guard below, the rest
    /// reserved; a touch anywhere below commits up to the committed part
    /// and moves the guard; the bottom pages raise the exhaustion once.
    /// A thread's end gives back its sparse stack while another thread
    /// allocates (CI once: a new thread's stack at the ended one's address
    /// lost its pages, as the old region's page states were written "not
    /// present" after the memory was already handed out). Page states go
    /// first now (`release_thread_memory`, `DosFreeMem`), so a block the
    /// other thread is given keeps its pages present.
    #[test]
    fn a_freed_stack_never_unmaps_a_block_given_out_meanwhile() {
        use super::super::Loader;
        const STACK: u32 = 8 * PAGE_SIZE;
        let l = Loader::new_mock();
        l.ensure_process_tables();
        let present = |page: u32| process_pte_at(page).and_then(|at| l.guest_read::<u64>(at)).is_some_and(|e| e & 1 != 0);
        let lost = std::sync::atomic::AtomicU32::new(0);
        std::thread::scope(|s| {
            s.spawn(|| for _ in 0..30000 {
                let Some(stack) = l.shared.mem_mgr.lock().unwrap().alloc(STACK) else { continue };
                let tib = l.shared.mem_mgr.lock().unwrap().alloc(PAGE_SIZE).unwrap();
                l.sparse_thread_stack(stack, STACK);
                l.release_thread_memory(stack, tib, 0);
            });
            s.spawn(|| for _ in 0..30000 {
                let Some(b) = l.shared.mem_mgr.lock().unwrap().alloc(STACK) else { continue };
                l.sync_allocations();
                // The top page: the one a sparse stack commits, and so the one
                // a late write of its states would unmap.
                for _ in 0..20 {
                    if !present(b + STACK - PAGE_SIZE) { lost.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                    std::hint::spin_loop();
                }
                l.shared.mem_mgr.lock().unwrap().free(b);
            });
        });
        assert_eq!(lost.load(std::sync::atomic::Ordering::Relaxed), 0, "a block given out lost its pages");
    }

    /// The same for `DosFreeMem`: a committed block's page states go
    /// before the block does, so a block given out meanwhile at its
    /// address keeps its pages.
    #[test]
    fn dos_free_mem_never_unmaps_a_block_given_out_meanwhile() {
        use super::super::Loader;
        const SIZE: u32 = 8 * PAGE_SIZE;
        let l = Loader::new_mock();
        l.ensure_process_tables();
        let present = |page: u32| process_pte_at(page).and_then(|at| l.guest_read::<u64>(at)).is_some_and(|e| e & 1 != 0);
        let lost = std::sync::atomic::AtomicU32::new(0);
        let ppb = 0x0010_0000;
        std::thread::scope(|s| {
            s.spawn(|| for _ in 0..30000 {
                if l.dos_alloc_mem(ppb, SIZE, PAG_COMMIT | PAG_READ | PAG_WRITE) != 0 { continue; }
                let a = l.guest_read::<u32>(ppb).unwrap();
                assert_eq!(l.dos_free_mem(a), 0);
            });
            s.spawn(|| for _ in 0..30000 {
                let Some(b) = l.shared.mem_mgr.lock().unwrap().alloc(SIZE) else { continue };
                l.sync_allocations();
                for _ in 0..20 {
                    if !present(b) { lost.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                    std::hint::spin_loop();
                }
                l.shared.mem_mgr.lock().unwrap().free(b);
            });
        });
        assert_eq!(lost.load(std::sync::atomic::Ordering::Relaxed), 0, "a block given out lost its pages");
    }

    #[test]
    fn a_sparse_stack_grows_down_and_runs_out_once() {
        let mut map = PageMap::default();
        assert!(map.allocate_stack(BASE, 3 * PAGE_SIZE).is_empty(), "too small: committed whole");
        assert!(!map.contains(BASE));
        let changes = map.allocate_stack(BASE, 8 * PAGE_SIZE);
        assert_eq!(changes.len(), 8);
        assert_eq!(changes[7], (BASE + 7 * PAGE_SIZE, PageAccess::ReadWrite), "the top page present");
        assert!(changes[..7].iter().all(|c| c.1 == PageAccess::None), "the rest not present");
        let rw = PAG_READ | PAG_WRITE;
        assert_eq!(map.query(BASE + 7 * PAGE_SIZE, PAGE_SIZE), Some((PAGE_SIZE, rw | PAG_COMMIT)));
        assert_eq!(map.query(BASE + 6 * PAGE_SIZE, PAGE_SIZE), Some((PAGE_SIZE, rw | PAG_COMMIT | PAG_GUARD)));
        assert_eq!(map.query(BASE, 6 * PAGE_SIZE), Some((6 * PAGE_SIZE, rw | PAG_BASE)), "reserved");
        // A frame larger than a page: a touch two pages below the guard.
        let (kind, changes) = map.fault(BASE + 4 * PAGE_SIZE + 16, true, false).unwrap();
        assert_eq!(kind, FaultKind::Retry);
        assert_eq!(changes.iter().map(|c| c.0).collect::<Vec<_>>(),
                   vec![BASE + 4 * PAGE_SIZE, BASE + 5 * PAGE_SIZE, BASE + 6 * PAGE_SIZE]);
        assert_eq!(map.query(BASE + 3 * PAGE_SIZE, PAGE_SIZE), Some((PAGE_SIZE, rw | PAG_COMMIT | PAG_GUARD)), "the new guard");
        assert_eq!(map.query(BASE + 4 * PAGE_SIZE, 0x10000), Some((4 * PAGE_SIZE, rw | PAG_COMMIT)));
        assert_eq!(map.fault(BASE + 2 * PAGE_SIZE, false, false).unwrap().0, FaultKind::Retry);
        assert_eq!(map.fault(BASE + PAGE_SIZE, false, false).unwrap().0, FaultKind::StackExhausted, "the bottom pages");
        assert_eq!(map.fault(BASE, false, false).unwrap().0, FaultKind::Retry, "once");
        assert_eq!(map.query(BASE, 0x10000), Some((8 * PAGE_SIZE, rw | PAG_COMMIT | PAG_BASE)), "all committed now");
        assert_eq!(map.free(BASE).unwrap().len(), 8, "every page not present again");
    }

    #[test]
    fn faults_retry_take_a_guard_or_violate() {
        let mut map = PageMap::default();
        map.allocate(BASE, 3 * PAGE_SIZE, PAG_READ | PAG_WRITE);
        map.set(BASE, PAGE_SIZE, PAG_COMMIT | PAG_READ).unwrap().unwrap();
        map.set(BASE + PAGE_SIZE, PAGE_SIZE, RW | PAG_GUARD).unwrap().unwrap();
        assert_eq!(map.fault(BASE + 8, false, false), Some((FaultKind::Retry, vec![(BASE, PageAccess::Read)])), "reading a readable page: a stale TLB");
        assert_eq!(map.fault(BASE + 8, true, false), Some((FaultKind::Violation, vec![])), "writing a read-only page");
        assert_eq!(map.fault(BASE + PAGE_SIZE, true, false), Some((FaultKind::Guard, vec![(BASE + PAGE_SIZE, PageAccess::ReadWrite)])));
        assert_eq!(map.fault(BASE + PAGE_SIZE, true, false), Some((FaultKind::Retry, vec![(BASE + PAGE_SIZE, PageAccess::ReadWrite)])), "the guard is gone");
        assert_eq!(map.fault(BASE + 2 * PAGE_SIZE, false, false), Some((FaultKind::Violation, vec![])), "reserved");
        assert_eq!(map.fault(BASE + 3 * PAGE_SIZE, false, false), None, "untracked");
    }

    /// Instruction fetches (ADR-0131): a committed page without
    /// PAG_EXECUTE is a violation, never a stale entry, a guard or a
    /// stack to grow; with PAG_EXECUTE the fetch runs again.
    #[test]
    fn fetches_need_pag_execute() {
        let mut map = PageMap::default();
        map.allocate(BASE, 2 * PAGE_SIZE, PAG_READ | PAG_WRITE);
        map.set(BASE, 2 * PAGE_SIZE, RW).unwrap().unwrap();
        assert_eq!(map.fault(BASE + 8, false, true), Some((FaultKind::Violation, vec![])), "read-write, no PAG_EXECUTE");
        let out = map.set(BASE + PAGE_SIZE, PAGE_SIZE, PAG_READ | PAG_EXECUTE).unwrap().unwrap();
        assert_eq!(out.changes, vec![(BASE + PAGE_SIZE, PageAccess::ReadExec)]);
        assert!(out.downgrade, "losing write");
        assert_eq!(map.fault(BASE + PAGE_SIZE, false, true), Some((FaultKind::Retry, vec![(BASE + PAGE_SIZE, PageAccess::ReadExec)])));
        let out = map.set(BASE + PAGE_SIZE, PAGE_SIZE, PAG_READ).unwrap().unwrap();
        assert!(out.downgrade, "losing execute flushes the TLBs");
        // A sparse stack's committed top page: a fetch does not grow it.
        let stack = BASE + 0x10_0000;
        map.allocate_stack(stack, 8 * PAGE_SIZE);
        assert_eq!(map.fault(stack + 7 * PAGE_SIZE, false, true), Some((FaultKind::Violation, vec![])));
    }
}
