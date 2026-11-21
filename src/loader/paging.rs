// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Guest paging for thread-local memory (ADR-0088).
//!
//! 32-bit guest threads run with 32-bit paging (`CR0.PG`, `CR4.PSE`):
//! - each thread has its own block of four pages: a page directory, its
//!   TLM table, its thread-local page and its ring-0 page (its TSS and
//!   ring-0 stack, ADR-0113);
//! - the directory maps the whole 4 GB linear space one-to-one with 4 MB
//!   pages, except the 4 MB holding `TLM_PAGE` (in the system area,
//!   ADR-0094), which go through the TLM table, and the program area and
//!   the allocator's region, which go through the process tables every
//!   thread shares (ADR-0114); the 4 MB after the TLM table's are paging
//!   arena, a supervisor-only large page (ADR-0115);
//! - the TLM table maps its 4 MB one-to-one too, except `TLM_PAGE`, which
//!   leads to the thread's own page, and the FS and ring-0 windows, which
//!   lead to its TIB's page and its ring-0 page;
//! - the process tables map their pages one-to-one, but for the low 64 KB,
//!   which are not present, as on OS/2 (a null pointer, or one just past
//!   it, traps as an access violation, ADR-0110), and the page states of
//!   allocated memory and of the program's objects (`vmem.rs`).
//!
//! So every address means what it meant unpaged but the low 64 KB, and
//! `TLM_PAGE` holds each thread's own words. EPT / NPT still bound the guest to its memory:
//! tables a guest damages can only map memory it could already reach.
//! The host side translates `TLM_PAGE` the same way for the thread a vCPU
//! thread runs (`guest_mem::set_thread_tlm_backing`).
//!
//! Protection (ADR-0113): every guest thread runs at ring 3. Every block
//! lies in the paging arena, whose pages, the GDT, IDT and stubs, and the
//! ring-0 window are supervisor-only in every thread's tables, and the API
//! thunks' 4 MB are read-only (`CR0.WP` holds Warpine's ring-0 stubs to
//! that too). A guest therefore cannot reach its tables, its TSS or
//! Warpine's descriptor tables, and `mov cr3` / `mov cr0` fault as
//! privileged instructions (`ring.rs`). Warpine itself does not write
//! those pages through an address a guest gave (`guest_mem::protected`).
//!
//! Security:
//! - the host never walks the guest's tables: `guest_mem` translates only
//!   `TLM_PAGE`, to the page Warpine itself chose for the vCPU thread when
//!   the thread started; every access stays bounds-checked;
//! - a block address is used only when it is one of the arena's blocks
//!   ([`valid_block`]), with checked arithmetic;
//! - crash reports say whether paging is on and whether CR3 is the
//!   thread's own block ([`PagingStatus`]).
//!
//! `DosAllocThreadLocalMemory` / `DosFreeThreadLocalMemory` (Control
//! Program Programming Reference) hand out words of the 32-DWORD
//! thread-local area at the start of that page, 1 to 8 per call; the
//! same words are reserved in every thread.

use std::sync::atomic::Ordering;

use super::constants::*;
use super::mutex_ext::MutexExt;
use log::{debug, warn};

/// Pages in a thread's paging block: directory, TLM table, thread-local
/// page, ring-0 page. (The first 4 MB go through a process table shared by
/// every thread, ADR-0114.)
pub const PAGING_BLOCK_PAGES: u32 = 4;
/// Bytes of a thread's paging block.
pub const PAGING_BLOCK_SIZE: u32 = PAGING_BLOCK_PAGES * PAGE_SIZE;
/// Offsets of the parts in the block.
const TABLE_OFFSET: u32 = PAGE_SIZE;
const TLM_OFFSET: u32 = 2 * PAGE_SIZE;
const RING0_OFFSET: u32 = 3 * PAGE_SIZE;
/// The low addresses no 32-bit thread reaches (OS/2's low 64 KB, below
/// every program's objects).
pub const LOW_GUARD: u32 = EXE_AREA_START;

/// The paging arena (ADR-0113, ADR-0115): from the page after the ring-0
/// window to the allocator's region: the rest of the 4 MB the TLM table
/// maps, then the next 4 MB ([`ARENA_CHUNK`]), a supervisor-only large
/// page. Its pages are supervisor-only in every thread's tables, so a
/// guest at ring 3 reaches nothing in it. It starts with the process's
/// page tables (ADR-0114), then holds every thread's paging block.
pub const PAGING_ARENA_BASE: u32 = RING0_WINDOW + PAGE_SIZE;
pub const PAGING_ARENA_END: u32 = (ARENA_CHUNK + 1) << LARGE_PAGE_SHIFT;
/// The 4 MB after the TLM table's, all of them paging arena (ADR-0115).
const ARENA_CHUNK: u32 = TLM_CHUNK + 1;
const _: () = assert!(PAGING_ARENA_END == DYNAMIC_ALLOC_BASE);
/// The allocator's region in 4 MB chunks: the directory entries that lead
/// to the process tables instead of large pages (ADR-0114).
pub const ALLOC_FIRST_CHUNK: u32 = DYNAMIC_ALLOC_BASE >> LARGE_PAGE_SHIFT;
pub const ALLOC_CHUNKS: u32 = (SHARED_ARENA_BASE - DYNAMIC_ALLOC_BASE) >> LARGE_PAGE_SHIFT;
const _: () = assert!(DYNAMIC_ALLOC_BASE.trailing_zeros() >= LARGE_PAGE_SHIFT
    && SHARED_ARENA_BASE.trailing_zeros() >= LARGE_PAGE_SHIFT);
/// The program area in 4 MB chunks (0 up to the API thunks), mapped
/// through process tables too, so the program's objects can be read-only
/// (ADR-0114); the first holds the not-present low 64 KB (ADR-0110).
pub const EXE_CHUNKS: u32 = EXE_AREA_END >> LARGE_PAGE_SHIFT;
const _: () = assert!(EXE_AREA_END.trailing_zeros() >= LARGE_PAGE_SHIFT && EXE_CHUNKS == STUB_CHUNK);
/// The process tables: one page table per chunk of the program area, then
/// one per allocator chunk, shared by all of a process's threads, so a
/// change of a page's state reaches them all.
pub const PROCESS_TABLES: u32 = PAGING_ARENA_BASE;
/// The allocator's tables, after the program area's.
pub const ALLOC_TABLES: u32 = PROCESS_TABLES + EXE_CHUNKS * PAGE_SIZE;
pub const PROCESS_TABLES_SIZE: u32 = (EXE_CHUNKS + ALLOC_CHUNKS) * PAGE_SIZE;
/// Where the paging blocks start, after the process tables.
pub const PAGING_BLOCKS_BASE: u32 = PROCESS_TABLES + PROCESS_TABLES_SIZE;
/// Blocks the arena holds: the most threads a process can have (482).
pub const PAGING_BLOCKS: u32 = (PAGING_ARENA_END - PAGING_BLOCKS_BASE) / PAGING_BLOCK_SIZE;
const _: () = assert!(PAGING_ARENA_BASE >> LARGE_PAGE_SHIFT == TLM_CHUNK && PAGING_BLOCKS >= 480);
/// The 4 MB holding the API thunks (`MAGIC_API_BASE`): read-only, so no
/// guest rewrites a thunk (at ring 0 too, with CR0.WP).
const STUB_CHUNK: u32 = (MAGIC_API_BASE >> LARGE_PAGE_SHIFT) as u32;
const _: () = assert!(STUB_CHUNK != TLM_CHUNK && STUB_CHUNK != 0
    && (MAGIC_API_BASE + STUB_AREA_SIZE as u64 - 1) >> LARGE_PAGE_SHIFT == STUB_CHUNK as u64
    && EXE_AREA_END as u64 <= (STUB_CHUNK as u64) << LARGE_PAGE_SHIFT
    && DYNAMIC_ALLOC_BASE as u64 >= ((STUB_CHUNK + 1) as u64) << LARGE_PAGE_SHIFT);

/// Whether the page at linear `page` is supervisor-only (ADR-0113): the
/// GDT, the IDT and its stubs (the system area below the TIB), the
/// ring-0 window and the paging arena. A guest at ring 3 cannot read or
/// write them; the processor's own accesses (descriptor tables, the TSS,
/// the stubs at ring 0) are supervisor accesses.
pub fn supervisor_only(page: u32) -> bool {
    (SYSTEM_AREA_BASE..TIB_BASE).contains(&page)
        || page == RING0_WINDOW
        || (PAGING_ARENA_BASE..PAGING_ARENA_END).contains(&page)
}

/// What a process-table entry lets the guest do with its page (ADR-0114).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageAccess {
    /// Not present: reserved but not committed, or a guard page.
    None,
    Read,
    ReadWrite,
}

/// The process-table entry for the allocator page holding `addr`.
pub fn process_pte(addr: u32, access: PageAccess) -> u32 {
    let page = addr & !(PAGE_SIZE - 1);
    match access {
        PageAccess::None => page,
        PageAccess::Read => page | PTE_PRESENT | PTE_USER | PTE_ACCESSED,
        PageAccess::ReadWrite => page | PTE_FLAGS,
    }
}

/// Where the process-table entry of the page holding `addr` lies; None
/// outside the program area and the allocator's region.
pub fn process_pte_at(addr: u32) -> Option<u32> {
    if addr < EXE_AREA_END {
        Some(PROCESS_TABLES + (addr >> PAGE_SHIFT) * 4)
    } else {
        (DYNAMIC_ALLOC_BASE..SHARED_ARENA_BASE).contains(&addr)
            .then(|| ALLOC_TABLES + ((addr - DYNAMIC_ALLOC_BASE) >> PAGE_SHIFT) * 4)
    }
}

/// Which blocks of the paging arena are in use (pure bookkeeping).
#[derive(Debug)]
pub struct PagingArena {
    used: Vec<bool>,
}

impl Default for PagingArena {
    fn default() -> Self {
        PagingArena { used: vec![false; PAGING_BLOCKS as usize] }
    }
}

impl PagingArena {
    /// A free block's address, now in use; None when every block is.
    pub fn alloc(&mut self) -> Option<u32> {
        let slot = self.used.iter().position(|&u| !u)?;
        self.used[slot] = true;
        Some(PAGING_BLOCKS_BASE + slot as u32 * PAGING_BLOCK_SIZE)
    }

    /// Give back the block at `base`; false for an address that is no
    /// block in use.
    pub fn free(&mut self, base: u32) -> bool {
        if !valid_block(base) {
            return false;
        }
        let slot = ((base - PAGING_BLOCKS_BASE) / PAGING_BLOCK_SIZE) as usize;
        std::mem::replace(&mut self.used[slot], false)
    }
}

/// Page-directory and page-table entry bits (Intel SDM vol. 3, 4.3).
const PTE_PRESENT: u32 = 1 << 0;
const PTE_WRITABLE: u32 = 1 << 1;
const PTE_USER: u32 = 1 << 2;
/// Accessed and dirty: set from the start, so the processor never writes
/// the entries (and the integrity check sees what Warpine wrote).
const PTE_ACCESSED: u32 = 1 << 5;
const PTE_DIRTY: u32 = 1 << 6;
/// A directory entry mapping a 4 MB page (needs `CR4.PSE`).
const PDE_LARGE: u32 = 1 << 7;
const PTE_FLAGS: u32 = PTE_PRESENT | PTE_WRITABLE | PTE_USER | PTE_ACCESSED | PTE_DIRTY;
/// A supervisor-only page (ADR-0113).
const PTE_SUPERVISOR: u32 = PTE_FLAGS & !PTE_USER;
/// The read-only 4 MB of the API thunks (ADR-0113).
const PDE_READ_ONLY: u32 = PTE_FLAGS & !PTE_WRITABLE;
const ENTRIES: u32 = 1024;
const LARGE_PAGE_SHIFT: u32 = 22;
/// The 4 MB block (directory entry) holding `TLM_PAGE`: the one mapped
/// through the page table.
const TLM_CHUNK: u32 = TLM_PAGE >> LARGE_PAGE_SHIFT;
/// `TLM_PAGE`'s entry in the page table.
const TLM_ENTRY: u32 = (TLM_PAGE >> PAGE_SHIFT) & (ENTRIES - 1);
/// The FS window's entry in the page table (ADR-0110).
const FS_ENTRY: u32 = (FS_WINDOW >> PAGE_SHIFT) & (ENTRIES - 1);
/// The ring-0 window's entry in the page table (ADR-0113).
const RING0_ENTRY: u32 = (RING0_WINDOW >> PAGE_SHIFT) & (ENTRIES - 1);
// The two tables map different 4 MB, and the guard lies in the first.
const _: () = assert!(TLM_CHUNK != 0 && LOW_GUARD < (1 << LARGE_PAGE_SHIFT) && LOW_GUARD.is_multiple_of(PAGE_SIZE));
const PAGE_SHIFT: u32 = 12;

/// Control-register bits.
pub const CR0_PG: u64 = 1 << 31;
/// CR3's page-level write-through bit: Warpine toggles it to make KVM
/// flush a vCPU's TLB (ADR-0114); the directory's address is the rest.
pub const CR3_PWT: u64 = 1 << 3;
pub const CR3_ADDRESS: u64 = !0xFFF;
/// Write protect: ring 0 cannot write a read-only page either (ADR-0113).
pub const CR0_WP: u64 = 1 << 16;
pub const CR4_PSE: u64 = 1 << 4;

/// DWORDs in the thread-local area, and most per call.
pub const TLM_WORDS: u32 = 32;
// The runtime block (ADR-0107) lies past the words handed out.
const _: () = assert!(RUNTIME_BLOCK >= TLM_PAGE + 4 * TLM_WORDS);
pub const TLM_MAX_PER_CALL: u32 = 8;

/// The paging block's contents for a block at `base`, for the thread
/// whose TIB is at `tib` (its page shows through the FS window; 0: the
/// window is not present): the directory, the TLM table, a zeroed
/// thread-local page, and the ring-0 page with the thread's TSS.
pub fn paging_block(base: u32, tib: u32) -> Vec<u8> {
    let mut block = vec![0u8; PAGING_BLOCK_SIZE as usize];
    let mut put = |at: u32, entry: u32| {
        block[at as usize..at as usize + 4].copy_from_slice(&entry.to_le_bytes());
    };
    for i in 0..ENTRIES {
        if i == TLM_CHUNK {
            put(4 * i, (base + TABLE_OFFSET) | PTE_FLAGS);
        } else if i < EXE_CHUNKS {
            put(4 * i, (PROCESS_TABLES + i * PAGE_SIZE) | PTE_FLAGS);
        } else if i == STUB_CHUNK {
            put(4 * i, (i << LARGE_PAGE_SHIFT) | PDE_READ_ONLY | PDE_LARGE);
        } else if i == ARENA_CHUNK {
            put(4 * i, (i << LARGE_PAGE_SHIFT) | PTE_SUPERVISOR | PDE_LARGE);
        } else if (ALLOC_FIRST_CHUNK..ALLOC_FIRST_CHUNK + ALLOC_CHUNKS).contains(&i) {
            put(4 * i, (ALLOC_TABLES + (i - ALLOC_FIRST_CHUNK) * PAGE_SIZE) | PTE_FLAGS);
        } else {
            put(4 * i, (i << LARGE_PAGE_SHIFT) | PTE_FLAGS | PDE_LARGE);
        }
    }
    for j in 0..ENTRIES {
        let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
        let target = match j {
            TLM_ENTRY => base + TLM_OFFSET,
            RING0_ENTRY => base + RING0_OFFSET,
            _ => page,
        };
        let flags = if supervisor_only(page) { PTE_SUPERVISOR } else { PTE_FLAGS };
        if j != FS_ENTRY {
            put(TABLE_OFFSET + 4 * j, target | flags);
        } else if tib != 0 {
            put(TABLE_OFFSET + 4 * j, (tib & !(PAGE_SIZE - 1)) | PTE_FLAGS);
        }
    }
    put(RING0_OFFSET + TSS_ESP0_OFFSET, RING0_STACK_TOP);
    put(RING0_OFFSET + TSS_SS0_OFFSET, u32::from(KERNEL_DATA_SEL));
    let iomap = (RING0_OFFSET + TSS_IOMAP_OFFSET) as usize;
    block[iomap..iomap + 2].copy_from_slice(&(TSS_SIZE as u16).to_le_bytes());
    block
}

/// Whether `base` can be a paging block: one of the paging arena's
/// blocks ([`PAGING_BLOCKS_BASE`], every [`PAGING_BLOCK_SIZE`] bytes).
pub fn valid_block(base: u32) -> bool {
    base.checked_sub(PAGING_BLOCKS_BASE)
        .is_some_and(|off| off.is_multiple_of(PAGING_BLOCK_SIZE) && off / PAGING_BLOCK_SIZE < PAGING_BLOCKS)
}

/// The thread-local page of the paging block at `cr3`; None for an
/// address that cannot be a block.
pub fn tlm_page_of(cr3: u64) -> Option<u32> {
    let base = u32::try_from(cr3 & CR3_ADDRESS).ok().filter(|&b| valid_block(b))?;
    Some(base + TLM_OFFSET)
}

/// The ring-0 page (TSS and ring-0 stack, ADR-0113) of the block whose
/// thread-local page is `page`; None for a page of no block.
pub fn ring0_page_of_tlm(page: u32) -> Option<u32> {
    block_of_tlm_page(page).map(|base| base + RING0_OFFSET)
}

/// The paging block whose thread-local page is `page`.
pub fn block_of_tlm_page(page: u32) -> Option<u32> {
    page.checked_sub(TLM_OFFSET).filter(|&b| valid_block(b))
}

/// The paging state of a vCPU, for crash reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingStatus {
    /// CR0.PG is off (the processor was reset, by a triple fault).
    Off,
    /// CR3 is the thread's own block.
    Own,
    /// CR3 is not the block Warpine gave the thread.
    ForeignCr3(u64),
}

impl std::fmt::Display for PagingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PagingStatus::Off => write!(f, "off"),
            PagingStatus::Own => write!(f, "on, the thread's own tables"),
            PagingStatus::ForeignCr3(cr3) => write!(f, "on, CR3 0x{cr3:08X} is not the thread's block"),
        }
    }
}

/// Which words of the thread-local area are handed out (pure bookkeeping).
#[derive(Debug, Default)]
pub struct TlmArea {
    /// Bit n: word n is in use.
    used: u32,
    /// The length of each block handed out, by its first word.
    blocks: [u8; TLM_WORDS as usize],
}

impl TlmArea {
    /// Hand out `count` consecutive words: the first word's index.
    pub fn alloc(&mut self, count: u32) -> Result<u32, u32> {
        if count == 0 || count > TLM_MAX_PER_CALL {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let run = (1u32 << count) - 1;
        let start = (0..=TLM_WORDS - count)
            .find(|&i| self.used & (run << i) == 0)
            .ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
        self.used |= run << start;
        self.blocks[start as usize] = count as u8;
        Ok(start)
    }

    /// Give back the block starting at word `start`.
    pub fn free(&mut self, start: u32) -> Result<(), u32> {
        let count = *self.blocks.get(start as usize).ok_or(ERROR_INVALID_PARAMETER)? as u32;
        if count == 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        self.used &= !(((1u32 << count) - 1) << start);
        self.blocks[start as usize] = 0;
        Ok(())
    }
}

impl super::Loader {
    /// The paging state of a vCPU with `sregs` on this host thread (whose
    /// block `guest_mem` knows from `run_vcpu`), for crash reports.
    pub(crate) fn paging_status(&self, sregs: &super::vm_backend::GuestSregs) -> PagingStatus {
        if sregs.cr0 & CR0_PG == 0 {
            return PagingStatus::Off;
        }
        match block_of_tlm_page(super::guest_mem::thread_tlm_backing()) {
            Some(base) if sregs.cr3 & CR3_ADDRESS == base as u64 => PagingStatus::Own,
            _ => PagingStatus::ForeignCr3(sregs.cr3),
        }
    }

    /// Allocate and fill a thread's paging block; its address (the CR3
    /// value), or None when there is no memory.
    pub(crate) fn alloc_paging_block(&self, tib: u32) -> Option<u32> {
        self.ensure_process_tables();
        let base = self.shared.paging_arena.lock_or_recover().alloc()?;
        if self.system_write_bytes(base, &paging_block(base, tib)).is_none() {
            self.shared.paging_arena.lock_or_recover().free(base);
            return None;
        }
        Some(base)
    }

    /// Write the process tables (ADR-0114) once, before the first thread's
    /// block or the first change of a page's state: the program area
    /// present and writable but the low 64 KB, the allocator's region not
    /// present; `vmem.rs` brings the allocations in and changes states.
    pub(crate) fn ensure_process_tables(&self) {
        if self.shared.process_tables_ready.swap(true, Ordering::AcqRel) {
            return;
        }
        // The program area's tables, the low 64 KB not present; then the
        // allocator's, not present until allocated (`vmem.rs` brings the
        // allocations in, those made before this too).
        let exe = (0..EXE_CHUNKS * ENTRIES).map(|i| i << PAGE_SHIFT)
            .map(|page| if page < LOW_GUARD { 0 } else { page | PTE_FLAGS });
        let alloc = (0..ALLOC_CHUNKS * ENTRIES).map(|i| DYNAMIC_ALLOC_BASE + (i << PAGE_SHIFT));
        let tables: Vec<u8> = exe.chain(alloc).flat_map(u32::to_le_bytes).collect();
        if self.system_write_bytes(PROCESS_TABLES, &tables).is_none() {
            warn!("the process page tables could not be written");
        }
    }

    /// Give back a thread's paging block (from [`Self::alloc_paging_block`]).
    pub(crate) fn free_paging_block(&self, base: u32) {
        if !self.shared.paging_arena.lock_or_recover().free(base) {
            warn!("paging block {base:#x} freed twice or never allocated");
        }
    }

    /// DosAllocThreadLocalMemory (ordinal 454): `cb` DWORDs (1–8) of the
    /// thread-local area, the same words in every thread; `*pp` gets their
    /// address.
    pub fn dos_alloc_thread_local_memory(&self, cb: u32, pp: u32) -> u32 {
        if pp == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let start = match self.shared.tlm.lock_or_recover().alloc(cb) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let addr = TLM_PAGE + 4 * start;
        debug!("  DosAllocThreadLocalMemory({}) -> {:#x}", cb, addr);
        if self.guest_write::<u32>(pp, addr).is_none() {
            let _ = self.shared.tlm.lock_or_recover().free(start);
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    /// DosFreeThreadLocalMemory (ordinal 455): give back the words `p`
    /// points at, as DosAllocThreadLocalMemory returned it.
    pub fn dos_free_thread_local_memory(&self, p: u32) -> u32 {
        let Some(inside) = p.checked_sub(TLM_PAGE).filter(|&o| o < 4 * TLM_WORDS && o % 4 == 0) else {
            return ERROR_INVALID_PARAMETER;
        };
        match self.shared.tlm.lock_or_recover().free(inside / 4) {
            Ok(()) => NO_ERROR,
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(block: &[u8], at: u32) -> u32 {
        u32::from_le_bytes(block[at as usize..at as usize + 4].try_into().unwrap())
    }

    /// Every address maps to itself, except the thread-local page.
    #[test]
    fn the_tables_map_everything_one_to_one_but_the_thread_local_page() {
        const BASE: u32 = PAGING_BLOCKS_BASE + 3 * PAGING_BLOCK_SIZE;
        let b = paging_block(BASE, TIB_BASE);
        assert_eq!(b.len(), PAGING_BLOCK_SIZE as usize);
        let chunk = TLM_CHUNK << LARGE_PAGE_SHIFT;
        assert_eq!(chunk, SYSTEM_AREA_BASE & !0x3F_FFFF, "the system area's 4 MB");
        assert_eq!(entry(&b, 4 * TLM_CHUNK), (BASE + TABLE_OFFSET) | PTE_FLAGS, "those 4 MB through the table");
        for i in 0..EXE_CHUNKS {
            assert_eq!(entry(&b, 4 * i), (PROCESS_TABLES + i * PAGE_SIZE) | PTE_FLAGS, "program area chunk {i}: a process table");
        }
        assert_eq!(entry(&b, 4 * (STUB_CHUNK + 1)), ((STUB_CHUNK + 1) << LARGE_PAGE_SHIFT) | PTE_FLAGS | PDE_LARGE);
        assert_eq!(entry(&b, 4 * 1023), 0xFFC0_0000 | PTE_FLAGS | PDE_LARGE);
        assert_eq!(entry(&b, TABLE_OFFSET), chunk | PTE_SUPERVISOR, "the GDT's first page: supervisor-only");
        let tib = (TIB_BASE >> PAGE_SHIFT) & (ENTRIES - 1);
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * tib), TIB_BASE | PTE_FLAGS, "the TIB");
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * TLM_ENTRY), (BASE + TLM_OFFSET) | PTE_FLAGS);
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * FS_ENTRY), TIB_BASE | PTE_FLAGS, "the FS window");
        assert_eq!(entry(&paging_block(BASE, 0), TABLE_OFFSET + 4 * FS_ENTRY), 0, "no TIB: no window");
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * 1023), (chunk + 0x003F_F000) | PTE_SUPERVISOR, "the arena's last page");
        assert!(b[TLM_OFFSET as usize..RING0_OFFSET as usize].iter().all(|&x| x == 0), "a zeroed thread-local page");
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * RING0_ENTRY), (BASE + RING0_OFFSET) | PTE_SUPERVISOR, "the ring-0 window");
        assert_eq!(entry(&b, 4 * STUB_CHUNK), (STUB_CHUNK << LARGE_PAGE_SHIFT) | PDE_READ_ONLY | PDE_LARGE, "the thunks: read-only");
        assert_eq!(entry(&b, 4 * ARENA_CHUNK), (ARENA_CHUNK << LARGE_PAGE_SHIFT) | PTE_SUPERVISOR | PDE_LARGE,
                   "the arena's second 4 MB: supervisor-only (ADR-0115)");
        assert_eq!(tlm_page_of(BASE as u64), Some(BASE + TLM_OFFSET));
    }

    /// No entry maps a page anywhere but to itself, except the
    /// thread-local page, which maps into the block itself, and the FS
    /// window, which shows the thread's own TIB: paging gives the guest no
    /// view it did not have unpaged.
    #[test]
    fn no_entry_maps_a_page_elsewhere_than_itself_but_the_thread_local_page() {
        const BASE: u32 = PAGING_BLOCKS_BASE;
        let b = paging_block(BASE, TIB_BASE);
        let alloc = ALLOC_FIRST_CHUNK..ALLOC_FIRST_CHUNK + ALLOC_CHUNKS;
        for i in alloc.clone() {
            assert_eq!(entry(&b, 4 * i), (ALLOC_TABLES + (i - ALLOC_FIRST_CHUNK) * PAGE_SIZE) | PTE_FLAGS,
                       "allocator chunk {i}: through its process table");
        }
        for i in (EXE_CHUNKS..ENTRIES).filter(|&i| i != TLM_CHUNK && !alloc.contains(&i)) {
            let e = entry(&b, 4 * i);
            assert_eq!(e & !0x3F_FFFF, i << LARGE_PAGE_SHIFT, "directory entry {i}");
            let flags = match i {
                STUB_CHUNK => PDE_READ_ONLY,
                ARENA_CHUNK => PTE_SUPERVISOR,
                _ => PTE_FLAGS,
            };
            assert_eq!(e & 0xFFF, flags | PDE_LARGE, "directory entry {i}");
        }
        for j in 0..ENTRIES {
            let e = entry(&b, TABLE_OFFSET + 4 * j);
            let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
            let flags = if supervisor_only(page) { PTE_SUPERVISOR } else { PTE_FLAGS };
            assert_eq!(e & 0xFFF, flags, "table entry {j}");
            let target = e & !0xFFF;
            if page == TLM_PAGE {
                assert_eq!(target, BASE + TLM_OFFSET);
                assert!(valid_block(target - TLM_OFFSET));
            } else if page == FS_WINDOW {
                assert_eq!(target, TIB_BASE, "the FS window shows the thread's TIB");
            } else if page == RING0_WINDOW {
                assert_eq!(target, BASE + RING0_OFFSET, "the ring-0 window shows the block's ring-0 page");
            } else {
                assert_eq!(target, page, "table entry {j}");
            }
        }
    }

    /// The ring-0 page starts with a TSS whose ring-0 stack is the rest of
    /// that page, on the ring-0 data selector, with no I/O bitmap; the
    /// stack itself starts zeroed (ADR-0113).
    #[test]
    fn the_ring0_page_holds_the_threads_tss_and_stack() {
        const BASE: u32 = PAGING_BLOCKS_BASE + 5 * PAGING_BLOCK_SIZE;
        let b = paging_block(BASE, TIB_BASE);
        let tss = |off: u32| entry(&b, RING0_OFFSET + off);
        assert_eq!(tss(TSS_ESP0_OFFSET), RING0_WINDOW + PAGE_SIZE, "ESP0: the top of the ring-0 page");
        assert_eq!(tss(TSS_SS0_OFFSET), u32::from(KERNEL_DATA_SEL), "SS0: ring-0 data");
        assert_eq!(tss(TSS_IOMAP_OFFSET - 2) >> 16, TSS_SIZE, "no I/O permission bitmap");
        assert_eq!(ring0_page_of_tlm(BASE + TLM_OFFSET), Some(BASE + RING0_OFFSET));
        assert_eq!(ring0_page_of_tlm(0), None);
        assert_eq!(KERNEL_DATA_SEL & 3, 0, "SS0's RPL is 0");
        let fields = [TSS_ESP0_OFFSET, TSS_SS0_OFFSET, TSS_IOMAP_OFFSET - 2];
        for off in (0..PAGE_SIZE).step_by(4).filter(|o| !fields.contains(o)) {
            assert_eq!(tss(off), 0, "ring-0 page offset {off:#x}");
        }
    }

    /// Only one of the paging arena's blocks is accepted, whatever a CR3
    /// value holds; nothing overflows.
    #[test]
    fn only_the_arenas_blocks_are_accepted() {
        let last = PAGING_BLOCKS_BASE + (PAGING_BLOCKS - 1) * PAGING_BLOCK_SIZE;
        assert!(valid_block(PAGING_BLOCKS_BASE));
        assert!(valid_block(last));
        assert!(last + PAGING_BLOCK_SIZE <= PAGING_ARENA_END);
        for bad in [0, TLM_PAGE, PAGING_ARENA_BASE, PROCESS_TABLES, PAGING_BLOCKS_BASE - PAGE_SIZE,
                    PAGING_BLOCKS_BASE + PAGE_SIZE, PAGING_BLOCKS_BASE + 1,
                    last + PAGING_BLOCK_SIZE, PAGING_ARENA_END, DYNAMIC_ALLOC_BASE, !0xFFF, u32::MAX] {
            assert!(!valid_block(bad), "{bad:#x}");
            // CR3's low 12 bits are not part of the address.
            if bad & 0xFFF == 0 {
                assert_eq!(tlm_page_of(bad as u64), None, "{bad:#x}");
            }
        }
        assert_eq!(tlm_page_of(1u64 << 32 | PAGING_BLOCKS_BASE as u64), None, "CR3 above 4 GB");
        assert_eq!(tlm_page_of(PAGING_BLOCKS_BASE as u64 | CR3_PWT), Some(PAGING_BLOCKS_BASE + TLM_OFFSET),
                   "CR3's write-through bit (a TLB flush) is not part of the address");
        assert_eq!(tlm_page_of(u64::MAX), None);
        assert_eq!(block_of_tlm_page(0), None);
        assert_eq!(block_of_tlm_page(u32::MAX), None);
        assert_eq!(block_of_tlm_page(PAGING_BLOCKS_BASE + TLM_OFFSET), Some(PAGING_BLOCKS_BASE));
    }

    /// The process tables (ADR-0114) map the program area (but the low
    /// 64 KB) present and writable, and the allocator's region page by page,
    /// present while allocated, and lie in the supervisor-only arena
    /// before the blocks; the entry helpers address and fill them.
    #[test]
    fn the_process_tables_map_the_allocators_region() {
        assert_eq!(ALLOC_CHUNKS * (1 << LARGE_PAGE_SHIFT), SHARED_ARENA_BASE - DYNAMIC_ALLOC_BASE);
        assert!(supervisor_only(PROCESS_TABLES) && supervisor_only(PAGING_BLOCKS_BASE - PAGE_SIZE));
        assert_eq!(process_pte_at(0), Some(PROCESS_TABLES), "the program area first");
        assert_eq!(process_pte_at(EXE_AREA_END - 1), Some(ALLOC_TABLES - 4));
        assert_eq!(process_pte_at(EXE_AREA_END), None, "the API thunks keep their large page");
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE), Some(ALLOC_TABLES));
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE + 0x234), Some(ALLOC_TABLES));
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE + 0x1234), Some(ALLOC_TABLES + 4));
        assert_eq!(process_pte_at(SHARED_ARENA_BASE - 1), Some(PAGING_BLOCKS_BASE - 4));
        assert_eq!(process_pte_at(SHARED_ARENA_BASE), None, "the shared arena keeps its large pages");
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE - 1), None);
        let page = DYNAMIC_ALLOC_BASE + 0x5000;
        assert_eq!(process_pte(page + 7, PageAccess::ReadWrite), page | PTE_FLAGS);
        assert_eq!(process_pte(page, PageAccess::Read), page | PTE_PRESENT | PTE_USER | PTE_ACCESSED);
        assert_eq!(process_pte(page, PageAccess::None) & PTE_PRESENT, 0);
        let loader = super::super::Loader::new_mock();
        loader.ensure_process_tables();
        for addr in [LOW_GUARD, EXE_AREA_START + 0x5000, EXE_AREA_END - PAGE_SIZE] {
            let at = process_pte_at(addr).unwrap();
            assert_eq!(loader.guest_read::<u32>(at), Some(addr | PTE_FLAGS), "{addr:#x}: the program area present");
        }
        for addr in [DYNAMIC_ALLOC_BASE, page, SHARED_ARENA_BASE - PAGE_SIZE] {
            let at = process_pte_at(addr).unwrap();
            assert_eq!(loader.guest_read::<u32>(at), Some(addr), "{addr:#x}: the allocator's region not present until allocated");
        }
        // An allocation comes in with the next sync, its free goes out.
        let block = loader.shared.mem_mgr.lock().unwrap().alloc(2 * PAGE_SIZE).unwrap();
        loader.sync_allocations();
        let entry = |a: u32| loader.guest_read::<u32>(process_pte_at(a).unwrap()).unwrap();
        assert_eq!((entry(block), entry(block + PAGE_SIZE)), (block | PTE_FLAGS, (block + PAGE_SIZE) | PTE_FLAGS));
        assert!(loader.shared.mem_mgr.lock().unwrap().free(block));
        loader.sync_allocations();
        assert_eq!(entry(block) & PTE_PRESENT, 0, "freed: not present");
        for addr in (0..LOW_GUARD).step_by(PAGE_SIZE as usize) {
            assert_eq!(loader.guest_read::<u32>(process_pte_at(addr).unwrap()).map(|e| e & PTE_PRESENT), Some(0),
                       "{addr:#x}: the low 64 KB not present");
        }
    }

    /// The arena hands out each block once, takes back only blocks in use,
    /// and runs out after PAGING_BLOCKS of them.
    #[test]
    fn the_arena_hands_out_each_block_once() {
        let mut arena = PagingArena::default();
        let blocks: Vec<u32> = std::iter::from_fn(|| arena.alloc()).collect();
        assert_eq!(blocks.len(), PAGING_BLOCKS as usize);
        assert!(blocks.iter().all(|&b| valid_block(b)));
        assert_eq!(blocks.iter().collect::<std::collections::HashSet<_>>().len(), blocks.len());
        assert!(arena.free(blocks[7]));
        assert!(!arena.free(blocks[7]), "not twice");
        assert!(!arena.free(DYNAMIC_ALLOC_BASE), "no block");
        assert_eq!(arena.alloc(), Some(blocks[7]), "a freed block comes back");
        assert_eq!(arena.alloc(), None);
    }

    /// The pages a guest at ring 3 must not reach (ADR-0113): the GDT, IDT
    /// and stubs, the ring-0 window and the arena; the TIB, PIB,
    /// environment, thread-local page, information segments and FS window
    /// stay the guest's.
    #[test]
    fn warpines_own_pages_are_supervisor_only() {
        for page in [SYSTEM_AREA_BASE, IDT_BASE, IDT_HANDLER_BASE & !0xFFF, TIB_BASE - PAGE_SIZE, RING0_WINDOW,
                     PAGING_ARENA_BASE, PAGING_ARENA_END - PAGE_SIZE] {
            assert!(supervisor_only(page), "{page:#x}");
        }
        for page in [TIB_BASE, PIB_BASE, ENV_ADDR, TLM_PAGE, super::super::infoseg::GINFOSEG_ADDR,
                     super::super::infoseg::LINFOSEG_ADDR, FS_WINDOW, PAGING_ARENA_END, MAGIC_API_BASE as u32] {
            assert!(!supervisor_only(page), "{page:#x}");
        }
    }

    /// The crash report's paging state: the thread's own block, paging
    /// off (after a reset), or a CR3 that is not its block.
    #[test]
    fn the_paging_state_names_the_threads_own_block() {
        let loader = super::super::Loader::new_mock();
        let base = loader.alloc_paging_block(TIB_BASE).expect("a paging block");
        let paged = |cr3: u64| super::super::vm_backend::GuestSregs { cr0: CR0_PG | 1, cr3, ..Default::default() };
        super::super::guest_mem::set_thread_tlm_backing(tlm_page_of(base as u64).unwrap());
        assert_eq!(loader.paging_status(&paged(base as u64)), PagingStatus::Own);
        assert_eq!(loader.paging_status(&paged(base as u64 | CR3_PWT)), PagingStatus::Own, "a TLB flush's CR3");
        assert_eq!(loader.paging_status(&Default::default()), PagingStatus::Off);
        assert_eq!(loader.paging_status(&paged(0x1234_5000)), PagingStatus::ForeignCr3(0x1234_5000));
        super::super::guest_mem::set_thread_tlm_backing(0);
    }

    /// Warpine's protected memory (the GDT, IDT and stubs, the paging
    /// arena) cannot be written through an ordinary write, which is what
    /// an API call writing through a guest's pointer does; Warpine's own
    /// system writes can. The thread-local page, whose backing lies in the
    /// arena, stays writable at `TLM_PAGE`.
    #[test]
    fn protected_memory_takes_only_system_writes() {
        use super::super::guest_mem::protected;
        let loader = super::super::Loader::new_mock();
        let base = loader.alloc_paging_block(TIB_BASE).expect("a paging block");
        for addr in [GDT_BASE, IDT_BASE, IDT_HANDLER_BASE, TIB_BASE - 4, PAGING_ARENA_BASE, base, base + TLM_OFFSET, PAGING_ARENA_END - 4] {
            assert!(protected(addr, 4), "{addr:#x}");
            assert_eq!(loader.guest_write::<u32>(addr, 0x5151), None, "{addr:#x}: refused");
            assert!(loader.guest_write_bytes(addr, &[1, 2, 3, 4]).is_none(), "{addr:#x}");
            assert!(loader.guest_slice_mut(addr, 4).is_none(), "{addr:#x}");
            assert!(loader.guest_read::<u32>(addr).is_some(), "{addr:#x}: reads stay possible");
        }
        assert!(protected(TIB_BASE - 2, 4), "a write that straddles the edge");
        for addr in [TIB_BASE, PIB_BASE, TLM_PAGE, FS_WINDOW, PAGING_ARENA_END, DYNAMIC_ALLOC_BASE] {
            assert!(!protected(addr, 4), "{addr:#x}");
        }
        assert_eq!(loader.system_write::<u32>(base + TLM_OFFSET, 0x5151), Some(()));
        super::super::guest_mem::set_thread_tlm_backing(base + TLM_OFFSET);
        assert_eq!(loader.guest_write::<u32>(TLM_PAGE + 8, 7), Some(()), "TLM_PAGE, translated into the arena");
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE), Some(0x5151));
        super::super::guest_mem::set_thread_tlm_backing(0);
    }

    #[test]
    fn thread_local_words_are_handed_out_and_given_back() {
        let mut a = TlmArea::default();
        assert_eq!(a.alloc(0), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(a.alloc(9), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(a.alloc(8), Ok(0));
        assert_eq!(a.alloc(1), Ok(8));
        assert_eq!(a.alloc(8), Ok(9));
        assert_eq!(a.alloc(8), Ok(17));
        assert_eq!(a.alloc(8), Err(ERROR_NOT_ENOUGH_MEMORY), "7 words left");
        assert_eq!(a.alloc(7), Ok(25));
        assert_eq!(a.alloc(1), Err(ERROR_NOT_ENOUGH_MEMORY));
        assert_eq!(a.free(8), Ok(()));
        assert_eq!(a.free(8), Err(ERROR_INVALID_PARAMETER), "freed already");
        assert_eq!(a.free(10), Err(ERROR_INVALID_PARAMETER), "not a block's start");
        assert_eq!(a.free(40), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(a.alloc(1), Ok(8), "the freed word again");
    }

    #[test]
    fn the_calls_check_their_arguments() {
        let loader = super::super::Loader::new_mock();
        const PP: u32 = 0x3000;
        assert_eq!(loader.dos_alloc_thread_local_memory(2, PP), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PP), Some(TLM_PAGE));
        assert_eq!(loader.dos_alloc_thread_local_memory(1, PP), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PP), Some(TLM_PAGE + 8));
        assert_eq!(loader.dos_alloc_thread_local_memory(1, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_free_thread_local_memory(TLM_PAGE + 2), ERROR_INVALID_PARAMETER, "misaligned");
        assert_eq!(loader.dos_free_thread_local_memory(TLM_PAGE + 4), ERROR_INVALID_PARAMETER, "inside a block");
        assert_eq!(loader.dos_free_thread_local_memory(0x1234), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_free_thread_local_memory(TLM_PAGE), NO_ERROR);
        assert_eq!(loader.dos_free_thread_local_memory(TLM_PAGE), ERROR_INVALID_PARAMETER);
    }

    /// The host side sees a vCPU thread's own page at `TLM_PAGE`.
    #[test]
    fn host_accesses_follow_the_thread_local_page() {
        let loader = super::super::Loader::new_mock();
        let block = loader.alloc_paging_block(TIB_BASE).expect("a paging block");
        let own = tlm_page_of(block as u64).unwrap();
        loader.guest_write::<u32>(TLM_PAGE, 0x1111).unwrap();
        super::super::guest_mem::set_thread_tlm_backing(own);
        super::super::guest_mem::set_thread_tib(TIB_BASE);
        loader.guest_write::<u32>(TLM_PAGE + 4, 0x2222).unwrap();
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE), Some(0), "this thread's page");
        assert_eq!(loader.guest_read::<u32>(own + 4), Some(0x2222));
        // An access that crosses the page's end is not translated.
        assert_eq!(loader.guest_read::<u64>(TLM_PAGE + PAGE_SIZE - 4).map(|v| v as u32), Some(0));
        super::super::guest_mem::set_thread_tlm_backing(0);
        super::super::guest_mem::set_thread_tib(0);
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE), Some(0x1111));
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE + 4), Some(0));
    }

    /// Host accesses near the thread-local page stay inside guest memory
    /// for any length, even with a page at the very end of it: no
    /// overflow, no panic, only None for what does not fit.
    #[test]
    fn translated_accesses_stay_bounded() {
        let loader = super::super::Loader::new_mock();
        let size = loader.shared.guest_mem.size() as u32;
        for backing in [size - PAGE_SIZE, size - 4, u32::MAX - 8, DYNAMIC_ALLOC_BASE] {
            super::super::guest_mem::set_thread_tlm_backing(backing);
            for (off, len) in [(TLM_PAGE, 0usize), (TLM_PAGE, 4), (TLM_PAGE + 4092, 4), (TLM_PAGE, PAGE_SIZE as usize),
                               (TLM_PAGE, PAGE_SIZE as usize + 1), (TLM_PAGE - 1, 2), (TLM_PAGE, usize::MAX),
                               (u32::MAX, 1), (u32::MAX - 3, 4)] {
                if let Some(p) = loader.guest_ptr(off, len) {
                    let start = loader.shared.guest_mem.host_base_addr() as usize;
                    let at = p as usize - start;
                    assert!(at.checked_add(len).is_some_and(|end| end <= size as usize),
                            "backing {backing:#x}: {off:#x}+{len} reached {at:#x}");
                }
            }
        }
        super::super::guest_mem::set_thread_tlm_backing(0);
        super::super::guest_mem::set_thread_tib(0);
    }
}
