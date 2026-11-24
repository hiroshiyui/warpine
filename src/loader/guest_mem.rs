// SPDX-License-Identifier: GPL-3.0-or-later
//
// Guest physical memory — sole access point for reads and writes into the
// KVM guest address space.  All operations are bounds-checked.  No other
// module may hold a raw `*mut u8` pointer into guest memory or perform direct
// pointer arithmetic on it outside of this file.

use std::ptr;
use super::MutexExt;

// ── The thread-local page ─────────────────────────────────────────────────────

thread_local! {
    /// The guest thread-local page of the guest thread this host thread
    /// runs (`paging.rs`); 0 for none. Host accesses that lie inside the
    /// page at `TLM_PAGE` go there, as the guest's own do through its page
    /// tables.
    static TLM_BACKING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Make this host thread's accesses to the thread-local page go to `page`
/// (0: to `TLM_PAGE` itself). Set by the vCPU thread before it runs.
pub fn set_thread_tlm_backing(page: u32) {
    TLM_BACKING.with(|b| b.set(page));
}

/// This host thread's thread-local page (0: none, the guest thread runs
/// unpaged).
pub fn thread_tlm_backing() -> u32 {
    TLM_BACKING.with(|b| b.get())
}

thread_local! {
    /// The TIB of the guest thread this host thread runs (0: none known):
    /// what `DosGetInfoBlocks` returns, and the page its tables' FS window
    /// shows (`paging.rs`, ADR-0110).
    static THREAD_TIB: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Record the TIB of the guest thread this host thread runs. Set by the
/// vCPU thread before it runs.
pub fn set_thread_tib(tib: u32) {
    THREAD_TIB.with(|t| t.set(tib));
}

/// The TIB of the guest thread this host thread runs (0: none known).
pub fn thread_tib() -> u32 {
    THREAD_TIB.with(|t| t.get())
}

/// The guest-physical address of an access of `len` bytes at `offset`: an
/// access wholly inside the thread-local page goes to this thread's own
/// page. One that only overlaps it is left as it is (unverified: the
/// guest's own access would cross into the next page).
fn tlm_translate(offset: u32, len: usize) -> u32 {
    use super::constants::{TLM_PAGE, PAGE_SIZE};
    let backing = TLM_BACKING.with(|b| b.get());
    match offset.checked_sub(TLM_PAGE) {
        // Saturating: a backing page near the top of the 32-bit space must
        // not wrap around to low memory (the bounds check that follows
        // then refuses it).
        Some(inside) if backing != 0 && (inside as usize).saturating_add(len) <= PAGE_SIZE as usize
            => backing.saturating_add(inside),
        _ => offset,
    }
}

// ── Warpine's protected memory ────────────────────────────────────────────────

/// Whether `len` bytes at the guest-physical `offset` touch Warpine's
/// protected memory (ADR-0113): the GDT, IDT and exception stubs (the
/// system area below the TIB) and the paging arena, both parts (page
/// tables, TSSs, ring-0 stacks). A guest at ring 3 cannot write them itself, and Warpine
/// does not write them on its behalf: every write through a guest-given
/// pointer goes through the checked paths, and only Warpine's own setup
/// uses the `*_system` ones. The address is the one given, before the
/// thread-local page is translated: `TLM_PAGE` stays writable, its backing
/// page in the arena does not.
pub fn protected(offset: u32, len: usize) -> bool {
    use super::constants::{SYSTEM_AREA_BASE, TIB_BASE};
    use super::paging::{PAGING_ARENA_BASE, PAGING_ARENA_END, PAGING_EXTRA_BASE, PAGING_EXTRA_END};
    let start = u64::from(offset);
    let end = start + len.max(1) as u64;
    let overlaps = |lo: u32, hi: u32| start < u64::from(hi) && end > u64::from(lo);
    overlaps(SYSTEM_AREA_BASE, TIB_BASE) || overlaps(PAGING_ARENA_BASE, PAGING_ARENA_END)
        || overlaps(PAGING_EXTRA_BASE, PAGING_EXTRA_END)
}

// ── GuestMemory ───────────────────────────────────────────────────────────────

/// Owns the `mmap`-backed host memory region that backs the KVM guest physical
/// address space.
///
/// Every read and write into guest memory **must** go through the methods on
/// this type; direct pointer arithmetic outside this file is not permitted.
/// This is the primary enforcement point for the hypervisor-escape prevention
/// rule: the guest can never cause the host to access memory outside this
/// allocation because every access is bounds-checked before dereferencing.
///
/// # Safety
///
/// `Send + Sync` is safe because:
/// - All public methods perform bounds checking before dereferencing.
/// - The `*mut u8` field is never exposed to callers.
/// - The allocation is exclusively owned by this type; no other object holds a
///   live pointer to the same region (the KVM kernel side accesses it via the
///   mapped physical region, which is controlled by the hypervisor, not a raw
///   pointer visible to safe Rust code).
pub struct GuestMemory {
    ptr:  *mut u8,
    size: usize,
    /// The shared-arena blocks this process may reach through a guest
    /// address (ADR-0049 addendum); `None`: no session arena, nothing to
    /// check.
    arena: std::sync::RwLock<Option<ArenaGuard>>,
}

/// A held block of the shared arena: guest addresses `start..end`, and
/// whether this process may write it.
pub type HeldBlock = (u32, u32, bool);

/// What [`GuestMemory`] knows of this process's holds in the shared arena.
pub struct ArenaGuard {
    /// The blocks held, as of the last look at the session's table.
    pub holds: Vec<HeldBlock>,
    /// Look again (the session's table, read without its lock: it is
    /// replaced by rename, so a read sees one whole version); `None` when
    /// it cannot be read, which keeps the holds known.
    pub refresh: Box<dyn Fn() -> Option<Vec<HeldBlock>> + Send + Sync>,
}

/// Whether `start..end` lies inside held blocks (writable ones for a
/// write); blocks may follow each other.
fn covered(holds: &[HeldBlock], start: u64, end: u64, write: bool) -> bool {
    let mut at = start;
    while at < end {
        match holds.iter().find(|&&(s, e, w)| u64::from(s) <= at && at < u64::from(e) && (w || !write)) {
            Some(&(_, e, _)) => at = u64::from(e),
            None => return false,
        }
    }
    true
}

unsafe impl Send for GuestMemory {}
unsafe impl Sync for GuestMemory {}

impl GuestMemory {
    /// Allocate `size` bytes of anonymous, zero-initialised memory for use as
    /// the KVM guest physical address space.
    ///
    /// Panics if `mmap` fails (unrecoverable — no guest memory → no execution).
    pub fn alloc(size: usize) -> Self {
        let raw = unsafe {
            libc::mmap(
                ptr::null_mut(), size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1, 0,
            )
        };
        if raw == libc::MAP_FAILED {
            panic!(
                "Failed to mmap {} bytes for guest memory: {}",
                size,
                std::io::Error::last_os_error()
            );
        }
        let p = raw as *mut u8;
        unsafe { ptr::write_bytes(p, 0, size); }
        GuestMemory { ptr: p, size, arena: std::sync::RwLock::new(None) }
    }

    /// Check guest addresses in the shared arena from now on (`guard`):
    /// an access through a guest address that touches the arena must lie
    /// in blocks the process holds (ADR-0049 addendum).
    pub fn guard_arena(&self, guard: ArenaGuard) {
        *self.arena.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(guard);
    }

    /// The blocks the process holds now (after a change of the table).
    pub fn set_arena_holds(&self, holds: Vec<HeldBlock>) {
        if let Some(g) = self.arena.write().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
            g.holds = holds;
        }
    }

    /// Whether a guest-address access of `len` bytes at `offset` may touch
    /// the shared arena: outside it, or inside blocks the process holds
    /// (writable ones for a write). A miss looks at the session's table
    /// again first: another process may have just given a block.
    pub fn arena_allows(&self, offset: u32, len: usize, write: bool) -> bool {
        use super::constants::{SHARED_ARENA_BASE, SHARED_ARENA_SIZE};
        let (lo, hi) = (u64::from(SHARED_ARENA_BASE), u64::from(SHARED_ARENA_BASE) + u64::from(SHARED_ARENA_SIZE));
        let (start, end) = (u64::from(offset), u64::from(offset).saturating_add(len.max(1) as u64));
        if end <= lo || start >= hi {
            return true;
        }
        let (start, end) = (start.max(lo), end.min(hi));
        let refresh = {
            let g = self.arena.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(guard) = g.as_ref() else { return true };
            if covered(&guard.holds, start, end, write) { return true; }
            (guard.refresh)()
        };
        let Some(holds) = refresh else { return false };
        let ok = covered(&holds, start, end, write);
        self.set_arena_holds(holds);
        ok
    }

    /// Total size of the guest memory region in bytes.
    pub fn size(&self) -> usize { self.size }

    /// Host virtual address of the base of the region.
    ///
    /// This value is passed to KVM (`KVM_SET_USER_MEMORY_REGION`) so the
    /// hypervisor knows where the guest physical pages live in host VA space.
    /// It must not be used to derive pointers for direct access — use
    /// [`ptr_at`] instead.
    pub fn host_base_addr(&self) -> u64 { self.ptr as u64 }

    /// Returns a checked raw pointer at `offset` for an access of `len` bytes,
    /// or `None` if `offset..offset+len` would exceed the allocation bounds.
    pub fn ptr_at(&self, offset: u32, len: usize) -> Option<*mut u8> {
        let off = tlm_translate(offset, len) as usize;
        if off.checked_add(len).is_none_or(|end| end > self.size) {
            return None;
        }
        Some(unsafe { self.ptr.add(off) })
    }

    /// Put the first `len` bytes of the file `fd` in place of the guest
    /// pages at `offset..offset + len`, shared (`MAP_SHARED | MAP_FIXED`):
    /// the shared arena (`shared_arena.rs`). The range must be page aligned
    /// and inside the allocation; the bounds every access checks stay the
    /// same, since the mapping replaces pages within them. The caller must
    /// pass a file that cannot shrink (a sealed memfd), so no access through
    /// this region can fault on a page past the file's end.
    pub fn map_shared(&self, offset: u32, len: usize, fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        const PAGE: usize = 4096;
        let off = offset as usize;
        if !off.is_multiple_of(PAGE) || !len.is_multiple_of(PAGE) || len == 0
            || off.checked_add(len).is_none_or(|end| end > self.size) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        // SAFETY: the target range lies inside this allocation (checked
        // above), which this type owns; MAP_FIXED replaces exactly those
        // pages and nothing else of the process. No reference into the range
        // is held across this call (all accesses copy through `ptr_at`).
        let raw = unsafe {
            libc::mmap(
                self.ptr.add(off) as *mut libc::c_void, len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(), 0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Read a `Copy` value from guest memory at `offset`.
    /// Returns `None` if the access would be out of bounds.
    pub fn read<T: Copy>(&self, offset: u32) -> Option<T> {
        if !self.arena_allows(offset, std::mem::size_of::<T>(), false) {
            return None;
        }
        let p = self.ptr_at(offset, std::mem::size_of::<T>())?;
        Some(unsafe { ptr::read_unaligned(p as *const T) })
    }

    /// Write a `Copy` value to guest memory at `offset`.
    /// Returns `None` if the access would be out of bounds, or would touch
    /// Warpine's protected memory ([`protected`]).
    pub fn write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        if protected(offset, std::mem::size_of::<T>()) || !self.arena_allows(offset, std::mem::size_of::<T>(), true) {
            return None;
        }
        self.write_system(offset, val)
    }

    /// Copy `data` into guest memory starting at `offset`.
    /// Returns `None` if the write would be out of bounds, or would touch
    /// Warpine's protected memory ([`protected`]).
    pub fn write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        if protected(offset, data.len()) || !self.arena_allows(offset, data.len(), true) {
            return None;
        }
        self.write_bytes_system(offset, data)
    }

    /// [`Self::write`] into Warpine's protected memory too: only for its
    /// own structures (descriptor tables, page tables), never at an
    /// address a guest gave.
    pub fn write_system<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        let p = self.ptr_at(offset, std::mem::size_of::<T>())?;
        // SAFETY: `ptr_at` checked that the whole value lies inside the
        // mapping; the write is unaligned-safe.
        unsafe { ptr::write_unaligned(p as *mut T, val); }
        Some(())
    }

    /// [`Self::write_bytes`] into Warpine's protected memory too (see
    /// [`Self::write_system`]).
    pub fn write_bytes_system(&self, offset: u32, data: &[u8]) -> Option<()> {
        let p = self.ptr_at(offset, data.len())?;
        // SAFETY: `ptr_at` checked that `data.len()` bytes from `p` lie
        // inside the mapping; `data` is host memory, so they do not overlap.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()); }
        Some(())
    }

    /// Copy `len` bytes out of guest memory; `None` if out of bounds.
    pub fn read_bytes(&self, offset: u32, len: usize) -> Option<Vec<u8>> {
        if !self.arena_allows(offset, len, false) {
            return None;
        }
        let p = self.ptr_at(offset, len)?;
        // SAFETY: `ptr_at` checked that `len` bytes from `p` lie inside
        // the mapping; they are copied out at once.
        Some(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
    }

    /// Return a mutable byte slice into guest memory at `offset..offset+len`.
    /// Returns `None` if the range would be out of bounds.
    ///
    /// # Safety
    ///
    /// This uses interior mutability through a raw pointer.  The caller must
    /// ensure no other live mutable reference into the same region exists.
    #[allow(clippy::mut_from_ref)]
    pub fn slice_mut(&self, offset: u32, len: usize) -> Option<&mut [u8]> {
        if protected(offset, len) || !self.arena_allows(offset, len, true) {
            return None;
        }
        let p = self.ptr_at(offset, len)?;
        Some(unsafe { std::slice::from_raw_parts_mut(p, len) })
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.size); }
    }
}

// ── Loader helper methods (thin wrappers over GuestMemory) ───────────────────

impl super::Loader {
    /// Returns a checked raw pointer into guest memory, or `None` if OOB.
    pub fn guest_ptr(&self, offset: u32, len: usize) -> Option<*mut u8> {
        if !self.shared.guest_mem.arena_allows(offset, len, false) {
            return None;
        }
        self.shared.guest_mem.ptr_at(offset, len)
    }

    /// Read a value from guest memory with bounds check.
    pub fn guest_read<T: Copy>(&self, offset: u32) -> Option<T> {
        self.shared.guest_mem.read::<T>(offset)
    }

    /// Write a value to guest memory with bounds check.
    pub fn guest_write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        self.shared.guest_mem.write::<T>(offset, val)
    }

    /// Copy bytes into guest memory with bounds check.
    pub fn guest_write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        self.shared.guest_mem.write_bytes(offset, data)
    }

    /// A buffer the guest gives an API call to read from (`DosWrite`,
    /// `DosWriteQueue`, …): its bytes, `None` when out of bounds, in
    /// Warpine's protected memory ([`protected`]: the guest cannot read
    /// that either) or in a shared block the process does not hold. A
    /// block held read-only is readable.
    pub fn guest_read_buffer(&self, offset: u32, len: usize) -> Option<Vec<u8>> {
        if protected(offset, len) {
            return None;
        }
        self.shared.guest_mem.read_bytes(offset, len)
    }

    /// Copy `len` bytes out of guest memory; `None` if out of bounds.
    pub fn guest_read_bytes(&self, offset: u32, len: usize) -> Option<Vec<u8>> {
        self.shared.guest_mem.read_bytes(offset, len)
    }

    /// Write into Warpine's protected memory (descriptor tables, page
    /// tables): only for Warpine's own structures, never at an address a
    /// guest gave (`GuestMemory::write_system`).
    pub(crate) fn system_write<T: Copy>(&self, offset: u32, val: T) -> Option<()> {
        self.shared.guest_mem.write_system::<T>(offset, val)
    }

    /// Bytes into Warpine's protected memory (see [`Self::system_write`]).
    pub(crate) fn system_write_bytes(&self, offset: u32, data: &[u8]) -> Option<()> {
        self.shared.guest_mem.write_bytes_system(offset, data)
    }

    /// Get a mutable slice of guest memory with bounds check.
    pub fn guest_slice_mut(&self, offset: u32, len: usize) -> Option<&mut [u8]> {
        self.shared.guest_mem.slice_mut(offset, len)
    }

    /// Allocate `size` bytes (whole pages) from the memory manager, all
    /// zero, as OS/2 commits memory: a reused block is cleared, so nothing
    /// an earlier allocation or module left shows through.
    pub fn alloc_zeroed(&self, size: u32) -> Option<u32> {
        let (addr, stale) = self.shared.mem_mgr.lock_or_recover().alloc_reporting(size)?;
        if stale > 0 {
            self.guest_slice_mut(addr, stale as usize)?.fill(0);
        }
        Some(addr)
    }

    /// Read a null-terminated string from guest memory, decoding it with the
    /// process's active codepage (`SharedState::active_codepage`).
    ///
    /// Bytes 0x00–0x7F are always pure ASCII. Upper bytes (0x80–0xFF) are
    /// decoded via `codepage::cp_decode` using the current process codepage so
    /// that non-ASCII characters (accented letters, box-drawing, etc.) are
    /// rendered as the correct Unicode codepoints rather than raw Latin-1 values.
    pub fn read_guest_string(&self, ptr: u32) -> String {
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        self.read_guest_string_cp(ptr, cp)
    }

    /// [`read_guest_string`](Self::read_guest_string) decoded with codepage
    /// `cp` (PM text uses the codepage of its message queue).
    pub fn read_guest_string_cp(&self, ptr: u32, cp: u32) -> String {
        const MAX_GUEST_STRING_LEN: usize = 4096;
        let base = ptr as usize;
        let mem_size = self.shared.guest_mem.size();
        if base >= mem_size { return String::new(); }
        let max_len = MAX_GUEST_STRING_LEN.min(mem_size - base);
        let mut bytes = Vec::with_capacity(64);
        for i in 0..max_len {
            let byte = self.shared.guest_mem.read::<u8>((base + i) as u32).unwrap_or(0);
            if byte == 0 { break; }
            bytes.push(byte);
        }
        super::codepage::cp_decode(&bytes, cp)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;

    /// The arena guard: blocks that follow each other cover a range
    /// together, a gap or a read-only block refuses it, addresses outside
    /// the arena are not checked, and no guard checks nothing.
    #[test]
    fn the_arena_guard_allows_only_held_ranges() {
        use super::{covered, ArenaGuard, GuestMemory};
        use super::super::constants::{SHARED_ARENA_BASE as A, SHARED_ARENA_SIZE};
        let held = [(A, A + 0x1000, true), (A + 0x1000, A + 0x2000, true), (A + 0x3000, A + 0x4000, false)];
        assert!(covered(&held, u64::from(A) + 0xFF0, u64::from(A) + 0x1010, true), "across two blocks");
        assert!(!covered(&held, u64::from(A) + 0x1FF0, u64::from(A) + 0x3010, false), "a gap");
        assert!(covered(&held, u64::from(A) + 0x3000, u64::from(A) + 0x3004, false));
        assert!(!covered(&held, u64::from(A) + 0x3000, u64::from(A) + 0x3004, true), "read-only");
        let gm = GuestMemory::alloc(0x1000);
        assert!(gm.arena_allows(A, 4, true), "no guard: not checked");
        gm.guard_arena(ArenaGuard { holds: held.to_vec(), refresh: Box::new(|| None) });
        assert!(gm.arena_allows(A + 4, 4, true));
        assert!(!gm.arena_allows(A + 0x2000, 4, false));
        assert!(gm.arena_allows(A - 8, 4, true) && gm.arena_allows(A + SHARED_ARENA_SIZE, 4, true), "outside");
        assert!(gm.arena_allows(A - 2, 4, false), "straddling its start into a held block");
        assert!(!gm.arena_allows(A + 0x1FFE, 4, false), "straddling into a gap");
        gm.set_arena_holds(Vec::new());
        assert!(!gm.arena_allows(A + 4, 4, false), "let go");
    }

    /// Memory handed out again is zero, as OS/2 commits it: DosAllocMem
    /// after DosFreeMem does not show the old contents; fresh memory is
    /// not touched.
    #[test]
    fn reused_memory_is_zeroed() {
        use super::super::MutexExt;
        let l = Loader::new_mock();
        let a = l.alloc_zeroed(8192).unwrap();
        l.guest_write_bytes(a, &[0xAB; 8192]).unwrap();
        assert!(l.shared.mem_mgr.lock_or_recover().free(a));
        // The same block again, cleared.
        let b = l.alloc_zeroed(5000).unwrap();
        assert_eq!(b, a);
        assert!(l.guest_slice_mut(b, 8192).unwrap().iter().all(|&x| x == 0));
        assert_eq!(l.shared.mem_mgr.lock_or_recover().alloc_reporting(4096).map(|r| r.1), Some(0), "fresh memory");
        // DosAllocMem the same way.
        l.guest_write_bytes(b, &[0xCD; 16]).unwrap();
        assert!(l.shared.mem_mgr.lock_or_recover().free(b));
        assert_eq!(l.dos_alloc_mem(0x3000, 16, super::super::constants::PAG_COMMIT | super::super::constants::PAG_READ | super::super::constants::PAG_WRITE), 0);
        let c = l.guest_read::<u32>(0x3000).unwrap();
        assert_eq!(c, b);
        assert!(l.guest_slice_mut(c, 16).unwrap().iter().all(|&x| x == 0));
    }

    #[test]
    fn test_mock_guest_read_write_u32() {
        let loader = Loader::new_mock();
        loader.guest_write::<u32>(0x100, 0xDEAD_BEEF).unwrap();
        assert_eq!(loader.guest_read::<u32>(0x100), Some(0xDEAD_BEEF));
    }

    #[test]
    fn test_mock_guest_read_write_u8_u16() {
        let loader = Loader::new_mock();
        loader.guest_write::<u8>(0x200, 0x42).unwrap();
        loader.guest_write::<u16>(0x202, 0x1234).unwrap();
        assert_eq!(loader.guest_read::<u8>(0x200), Some(0x42));
        assert_eq!(loader.guest_read::<u16>(0x202), Some(0x1234));
    }

    #[test]
    fn test_mock_guest_write_bytes_and_read_back() {
        let loader = Loader::new_mock();
        let data = b"Hello, OS/2!";
        loader.guest_write_bytes(0x300, data).unwrap();
        for (i, &b) in data.iter().enumerate() {
            assert_eq!(loader.guest_read::<u8>(0x300 + i as u32), Some(b));
        }
    }

    #[test]
    fn test_mock_guest_slice_mut() {
        let loader = Loader::new_mock();
        {
            let s = loader.guest_slice_mut(0x400, 4).unwrap();
            s.copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        }
        // Little-endian: bytes [01 02 03 04] → u32 0x04030201
        assert_eq!(loader.guest_read::<u32>(0x400), Some(0x0403_0201));
    }

    #[test]
    fn test_mock_guest_oob_read_returns_none() {
        let loader = Loader::new_mock();
        let end = 64 * 1024 * 1024u32; // 64 MB mock region (new_mock allocates 64 MB)
        assert!(loader.guest_read::<u8>(end - 1).is_some());   // last valid byte
        assert!(loader.guest_read::<u8>(end).is_none());        // one past end
        assert!(loader.guest_read::<u32>(end - 3).is_none());   // u32 straddles end
        assert!(loader.guest_write::<u32>(end - 3, 0).is_none());
    }

    #[test]
    fn test_mock_read_guest_string_null_terminated() {
        let loader = Loader::new_mock();
        loader.guest_write_bytes(0x500, b"OS/2 Warp\0extra").unwrap();
        assert_eq!(loader.read_guest_string(0x500), "OS/2 Warp");
    }

    #[test]
    fn test_mock_read_guest_string_oob_ptr() {
        let loader = Loader::new_mock();
        // Pointer past the end of memory → empty string, no panic
        assert_eq!(loader.read_guest_string(2 * 1024 * 1024), "");
    }
}
