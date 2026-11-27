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
//!   thread shares (ADR-0114); the 4 MB after the TLM table's, and the
//!   4 MB before it, are paging arena, each a supervisor-only large page
//!   (ADR-0115);
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

/// Pages in a thread's paging block: the first gigabyte's page directory,
/// with the thread's PDPT in its last 32 bytes, the TLM table, the
/// thread-local page and the ring-0 page (its TSS and ring-0 stack).
pub const PAGING_BLOCK_PAGES: u32 = 4;
/// Bytes of a thread's paging block.
pub const PAGING_BLOCK_SIZE: u32 = PAGING_BLOCK_PAGES * PAGE_SIZE;
/// Offsets of the parts in the block.
const DIRECTORY_OFFSET: u32 = 0;
const TABLE_OFFSET: u32 = PAGE_SIZE;
const TLM_OFFSET: u32 = 2 * PAGE_SIZE;
const RING0_OFFSET: u32 = 3 * PAGE_SIZE;
/// A PDPT's size: four 8-byte entries (ADR-0131).
const PDPT_SIZE: u32 = 32;
/// The PDPT's offset in the block: the directory's last four entries
/// (ADR-0131 addendum). Each thread's PDPT is in a page of its own, as
/// some nested hosts cache PDPTs by the page CR3 names; the entries it
/// takes would map 1016 MB - 1 GB, far above guest memory, and as
/// directory entries they name supervisor-only, read-only tables.
const PDPT_OFFSET: u32 = DIRECTORY_OFFSET + PAGE_SIZE - PDPT_SIZE;
const PDPT_FIRST_ENTRY: u32 = PDPT_OFFSET / ENTRY_BYTES;
const _: () = assert!(GUEST_CHUNKS <= PDPT_FIRST_ENTRY);
/// The low addresses no 32-bit thread reaches (OS/2's low 64 KB, below
/// every program's objects).
pub const LOW_GUARD: u32 = EXE_AREA_START;

/// The paging arena (ADR-0113, ADR-0115): from the page after the ring-0
/// window to the allocator's region: the rest of the 2 MB the TLM table
/// maps, then three more 2 MB chunks ([`ARENA_CHUNKS`]), supervisor-only
/// large pages. Its pages are supervisor-only in every thread's tables, so
/// a guest at ring 3 reaches nothing in it. It holds the first threads'
/// paging blocks; the rest lie in [`PAGING_EXTRA_BASE`].
pub const PAGING_ARENA_BASE: u32 = RING0_WINDOW + PAGE_SIZE;
pub const PAGING_ARENA_END: u32 = DYNAMIC_ALLOC_BASE;
/// The 2 MB chunks after the TLM table's, all of them paging arena.
const ARENA_CHUNKS: std::ops::Range<u32> = TLM_CHUNK + 1..DYNAMIC_ALLOC_BASE >> LARGE_PAGE_SHIFT;
/// The allocator's region in 2 MB chunks: the directory entries that lead
/// to the process tables instead of large pages (ADR-0114).
pub const ALLOC_FIRST_CHUNK: u32 = DYNAMIC_ALLOC_BASE >> LARGE_PAGE_SHIFT;
pub const ALLOC_CHUNKS: u32 = (SHARED_ARENA_BASE - DYNAMIC_ALLOC_BASE) >> LARGE_PAGE_SHIFT;
const _: () = assert!(DYNAMIC_ALLOC_BASE.trailing_zeros() >= LARGE_PAGE_SHIFT
    && SHARED_ARENA_BASE.trailing_zeros() >= LARGE_PAGE_SHIFT);
/// The program area in 2 MB chunks (0 up to the API thunks), mapped
/// through process tables too, so the program's objects can be read-only
/// (ADR-0114); the first holds the not-present low 64 KB (ADR-0110).
pub const EXE_CHUNKS: u32 = EXE_AREA_END >> LARGE_PAGE_SHIFT;
const _: () = assert!(EXE_AREA_END.trailing_zeros() >= LARGE_PAGE_SHIFT && EXE_CHUNKS == STUB_CHUNK);
/// The session's shared arena (ADR-0049) in 2 MB chunks: mapped through
/// process tables too, so a process reaches only the blocks it holds, with
/// the access it holds them with (ADR-0049 addendum, `shared_arena.rs`).
pub const SHARED_FIRST_CHUNK: u32 = SHARED_ARENA_BASE >> LARGE_PAGE_SHIFT;
pub const SHARED_CHUNKS: u32 = SHARED_ARENA_SIZE >> LARGE_PAGE_SHIFT;
const _: () = assert!(SHARED_ARENA_SIZE.trailing_zeros() >= LARGE_PAGE_SHIFT);
/// The guest's memory in 2 MB chunks: the first gigabyte's directory maps
/// these through tables or large pages, the rest of the 4 GB one-to-one.
const GUEST_CHUNKS: u32 = GUEST_MEM_SIZE >> LARGE_PAGE_SHIFT;
const _: () = assert!(SHARED_FIRST_CHUNK + SHARED_CHUNKS == GUEST_CHUNKS && GUEST_CHUNKS <= ENTRIES);
/// The 2 MB after the API thunks' (ADR-0131): the process tables, a
/// supervisor-only large page in every directory. One page table per chunk
/// of the program area, then one per allocator chunk, then one per chunk of
/// the shared arena, then the three directories of the 1-4 GB, all shared
/// by a process's threads, so a change of a page's state reaches them all.
const TABLES_CHUNK: u32 = STUB_CHUNK + 1;
pub const PROCESS_TABLES: u32 = TABLES_CHUNK << LARGE_PAGE_SHIFT;
const PROCESS_TABLES_END: u32 = (TABLES_CHUNK + 1) << LARGE_PAGE_SHIFT;
/// The allocator's tables, after the program area's.
pub const ALLOC_TABLES: u32 = PROCESS_TABLES + EXE_CHUNKS * PAGE_SIZE;
/// The shared arena's tables, after the allocator's.
pub const SHARED_TABLES: u32 = ALLOC_TABLES + ALLOC_CHUNKS * PAGE_SIZE;
/// The directories of PDPT entries 1-3 (1-4 GB, one-to-one large pages).
const HIGH_DIRECTORIES: u32 = SHARED_TABLES + SHARED_CHUNKS * PAGE_SIZE;
const HIGH_DIRECTORY_COUNT: u32 = 3;
pub const PROCESS_TABLES_SIZE: u32 = (EXE_CHUNKS + ALLOC_CHUNKS + SHARED_CHUNKS + HIGH_DIRECTORY_COUNT) * PAGE_SIZE;
const _: () = assert!(PROCESS_TABLES + PROCESS_TABLES_SIZE <= PROCESS_TABLES_END
    && PROCESS_TABLES as u64 >= MAGIC_API_BASE + STUB_AREA_SIZE as u64);
/// Where the paging blocks start: the whole arena holds blocks.
pub const PAGING_BLOCKS_BASE: u32 = PAGING_ARENA_BASE;
/// Blocks in the arena (495).
const MAIN_BLOCKS: u32 = (PAGING_ARENA_END - PAGING_BLOCKS_BASE) / PAGING_BLOCK_SIZE;
const _: () = assert!(PAGING_ARENA_BASE >> LARGE_PAGE_SHIFT == TLM_CHUNK && MAIN_BLOCKS >= 480);
/// The 4 MB between the process tables and the system area (ADR-0115):
/// more paging blocks, supervisor-only large pages in every directory.
const EXTRA_CHUNKS: std::ops::Range<u32> = TABLES_CHUNK + 1..TLM_CHUNK;
pub const PAGING_EXTRA_BASE: u32 = EXTRA_CHUNKS.start << LARGE_PAGE_SHIFT;
pub const PAGING_EXTRA_END: u32 = EXTRA_CHUNKS.end << LARGE_PAGE_SHIFT;
const _: () = assert!(PAGING_EXTRA_END == SYSTEM_AREA_BASE && PAGING_EXTRA_BASE == PROCESS_TABLES_END);
/// Blocks in the extra 4 MB (256).
const EXTRA_BLOCKS: u32 = (PAGING_EXTRA_END - PAGING_EXTRA_BASE) / PAGING_BLOCK_SIZE;
/// Blocks the arena holds: the most threads a process can have (751).
pub const PAGING_BLOCKS: u32 = MAIN_BLOCKS + EXTRA_BLOCKS;
const _: () = assert!(PAGING_BLOCKS >= 736);
/// The 2 MB holding the API thunks (`MAGIC_API_BASE`) and the exit trap:
/// read-only, so no guest rewrites a thunk (at ring 0 too, with CR0.WP),
/// and executable.
const STUB_CHUNK: u32 = (MAGIC_API_BASE >> LARGE_PAGE_SHIFT) as u32;
const _: () = assert!(STUB_CHUNK != TLM_CHUNK && STUB_CHUNK != 0
    && (MAGIC_API_BASE + STUB_AREA_SIZE as u64 - 1) >> LARGE_PAGE_SHIFT == STUB_CHUNK as u64
    && (EXIT_TRAP_ADDR >> LARGE_PAGE_SHIFT) == STUB_CHUNK
    && EXE_AREA_END as u64 <= (STUB_CHUNK as u64) << LARGE_PAGE_SHIFT
    && DYNAMIC_ALLOC_BASE as u64 >= ((STUB_CHUNK + 1) as u64) << LARGE_PAGE_SHIFT);

/// Whether the page at linear `page` is supervisor-only (ADR-0113): the
/// GDT, the IDT and its stubs (the system area below the TIB), the
/// ring-0 window, the process tables and the paging arena, both parts. A
/// guest at ring 3 cannot read or write them; the processor's own accesses
/// (descriptor tables, the TSS, the stubs at ring 0) are supervisor
/// accesses.
pub fn supervisor_only(page: u32) -> bool {
    (SYSTEM_AREA_BASE..TIB_BASE).contains(&page)
        || page == RING0_WINDOW
        || (PAGING_ARENA_BASE..PAGING_ARENA_END).contains(&page)
        || (PROCESS_TABLES..PAGING_EXTRA_END).contains(&page)
}

/// Whether the page at linear `page` holds Warpine's ring-0 code, the
/// IDT's stubs (from the IDT's page up to the TIB), which stays
/// executable (ADR-0131).
fn ring0_code(page: u32) -> bool {
    (IDT_BASE & !(PAGE_SIZE - 1)..TIB_BASE).contains(&page)
}

/// What a process-table entry lets the guest do with its page (ADR-0114,
/// ADR-0131).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageAccess {
    /// Not present: reserved but not committed, or a guard page.
    None,
    Read,
    ReadWrite,
    /// Read and execute.
    ReadExec,
    /// Read, write and execute.
    ReadWriteExec,
}

impl PageAccess {
    /// The access a page of these permissions gives.
    pub fn of(read: bool, write: bool, exec: bool) -> PageAccess {
        match (read || write || exec, write, exec) {
            (false, _, _) => PageAccess::None,
            (true, false, false) => PageAccess::Read,
            (true, true, false) => PageAccess::ReadWrite,
            (true, false, true) => PageAccess::ReadExec,
            (true, true, true) => PageAccess::ReadWriteExec,
        }
    }
    pub fn present(self) -> bool { self != PageAccess::None }
    pub fn writable(self) -> bool { matches!(self, PageAccess::ReadWrite | PageAccess::ReadWriteExec) }
    pub fn executable(self) -> bool { matches!(self, PageAccess::ReadExec | PageAccess::ReadWriteExec) }
    /// The same access, executable as well when `exec`.
    pub fn with_exec(self, exec: bool) -> PageAccess {
        if !self.present() { return PageAccess::None; }
        PageAccess::of(true, self.writable(), self.executable() || exec)
    }
    /// Whether going from `self` to `new` takes any permission away (a TLB
    /// may hold the old entry).
    pub fn loses_to(self, new: PageAccess) -> bool {
        (self.present() && !new.present()) || (self.writable() && !new.writable())
            || (self.executable() && !new.executable())
    }
}

/// Whether execute permission is enforced in this process (ADR-0131): on
/// for 32-bit processes unless `WARPINE_NX=off`; 16-bit processes turn it
/// off. One OS/2 process is one host process, so it is process-wide.
static NX_ENFORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The environment variable that turns execute protection off for a run.
pub const NX_ENV: &str = "WARPINE_NX";

/// Turn execute protection on or off for this process, before its first
/// thread's tables are written.
pub fn set_nx_enforced(on: bool) {
    NX_ENFORCED.store(on, Ordering::Relaxed);
}

pub fn nx_enforced() -> bool {
    NX_ENFORCED.load(Ordering::Relaxed)
}

/// Whether `WARPINE_NX`'s value leaves execute protection on: everything
/// but `off`, `0`, `no` or `false` (any case).
pub fn nx_setting(value: Option<&str>) -> bool {
    !value.is_some_and(|v| ["off", "0", "no", "false"].iter().any(|o| v.eq_ignore_ascii_case(o)))
}

/// The process-table entry for the page holding `addr`.
pub fn process_pte(addr: u32, access: PageAccess) -> u64 {
    process_pte_nx(addr, access, nx_enforced())
}

/// [`process_pte`] with execute protection on (`nx`) or off.
pub fn process_pte_nx(addr: u32, access: PageAccess, nx: bool) -> u64 {
    let page = u64::from(addr & !(PAGE_SIZE - 1));
    let xd = if nx && !access.executable() { PTE_XD } else { 0 };
    match access {
        PageAccess::None => page,
        PageAccess::Read | PageAccess::ReadExec => page | PTE_PRESENT | PTE_USER | PTE_ACCESSED | xd,
        PageAccess::ReadWrite | PageAccess::ReadWriteExec => page | PTE_FLAGS | xd,
    }
}

/// The access a process-table entry gives (the inverse of [`process_pte`]).
pub fn pte_access(entry: u64) -> PageAccess {
    PageAccess::of(entry & PTE_PRESENT != 0, entry & PTE_PRESENT != 0 && entry & PTE_WRITABLE != 0,
                   entry & PTE_PRESENT != 0 && entry & PTE_XD == 0)
}

/// Where the process-table entry of the page holding `addr` lies; None
/// outside the program area, the allocator's region and the shared arena
/// (whose tables follow the allocator's).
pub fn process_pte_at(addr: u32) -> Option<u32> {
    if addr < EXE_AREA_END {
        Some(PROCESS_TABLES + (addr >> PAGE_SHIFT) * ENTRY_BYTES)
    } else {
        (DYNAMIC_ALLOC_BASE..SHARED_ARENA_BASE + SHARED_ARENA_SIZE).contains(&addr)
            .then(|| ALLOC_TABLES + ((addr - DYNAMIC_ALLOC_BASE) >> PAGE_SHIFT) * ENTRY_BYTES)
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
        Some(block_base(slot as u32))
    }

    /// Give back the block at `base`; false for an address that is no
    /// block in use.
    pub fn free(&mut self, base: u32) -> bool {
        let Some(slot) = block_slot(base) else { return false };
        std::mem::replace(&mut self.used[slot as usize], false)
    }
}

/// PAE entry bits (Intel SDM vol. 3, 4.4).
const PTE_PRESENT: u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER: u64 = 1 << 2;
/// Accessed and dirty: set from the start, so the processor never writes
/// the entries (and the integrity check sees what Warpine wrote).
const PTE_ACCESSED: u64 = 1 << 5;
const PTE_DIRTY: u64 = 1 << 6;
/// A directory entry mapping a 2 MB page.
const PDE_LARGE: u64 = 1 << 7;
/// Execute-disable (needs `EFER.NXE`, ADR-0131).
const PTE_XD: u64 = 1 << 63;
const PTE_FLAGS: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_USER | PTE_ACCESSED | PTE_DIRTY;
/// A supervisor-only page (ADR-0113).
const PTE_SUPERVISOR: u64 = PTE_FLAGS & !PTE_USER;
/// The read-only 2 MB of the API thunks (ADR-0113).
const PDE_READ_ONLY: u64 = PTE_FLAGS & !PTE_WRITABLE;
/// A PDPT entry: present, nothing else (its other low bits are reserved).
const PDPTE_PRESENT: u64 = 1 << 0;
/// Entries per table, and their size.
const ENTRIES: u32 = 512;
const ENTRY_BYTES: u32 = 8;
const LARGE_PAGE_SHIFT: u32 = 21;
/// The 2 MB block (directory entry) holding `TLM_PAGE`: the one mapped
/// through the page table.
const TLM_CHUNK: u32 = TLM_PAGE >> LARGE_PAGE_SHIFT;
/// `TLM_PAGE`'s entry in the page table.
const TLM_ENTRY: u32 = (TLM_PAGE >> PAGE_SHIFT) & (ENTRIES - 1);
/// The FS window's entry in the page table (ADR-0110).
const FS_ENTRY: u32 = (FS_WINDOW >> PAGE_SHIFT) & (ENTRIES - 1);
/// The ring-0 window's entry in the page table (ADR-0113).
const RING0_ENTRY: u32 = (RING0_WINDOW >> PAGE_SHIFT) & (ENTRIES - 1);
// The table maps the system area, the TLM page and both windows; the
// guard lies in the first chunk.
const _: () = assert!(TLM_CHUNK != 0 && LOW_GUARD < (1 << LARGE_PAGE_SHIFT) && LOW_GUARD.is_multiple_of(PAGE_SIZE)
    && SYSTEM_AREA_BASE >> LARGE_PAGE_SHIFT == TLM_CHUNK && RING0_WINDOW >> LARGE_PAGE_SHIFT == TLM_CHUNK
    && FS_WINDOW >> LARGE_PAGE_SHIFT == TLM_CHUNK);
const PAGE_SHIFT: u32 = 12;

/// Control-register bits.
pub const CR0_PG: u64 = 1 << 31;
/// A bit CR3 ignores in PAE mode (the PDPT's address is bits 31:5):
/// Warpine toggles it to make KVM flush a vCPU's TLB (ADR-0114).
pub const CR3_PWT: u64 = 1 << 3;
/// The PDPT's address in CR3.
pub const CR3_ADDRESS: u64 = !0x1F;
/// Write protect: ring 0 cannot write a read-only page either (ADR-0113).
pub const CR0_WP: u64 = 1 << 16;
/// PAE paging (ADR-0131).
pub const CR4_PAE: u64 = 1 << 5;
/// EFER's execute-disable enable (ADR-0131).
pub const EFER_NXE: u64 = 1 << 11;

/// DWORDs in the thread-local area, and most per call.
pub const TLM_WORDS: u32 = 32;
// The runtime block (ADR-0107) lies past the words handed out.
const _: () = assert!(RUNTIME_BLOCK >= TLM_PAGE + 4 * TLM_WORDS);
pub const TLM_MAX_PER_CALL: u32 = 8;

/// The paging block's contents for a block at `base`, for the thread
/// whose TIB is at `tib` (its page shows through the FS window; 0: the
/// window is not present), with execute protection on (`nx`) or off: the
/// first gigabyte's directory, the TLM table, a zeroed thread-local page,
/// and the ring-0 page with the thread's TSS and PDPT.
pub fn paging_block(base: u32, tib: u32, nx: bool) -> Vec<u8> {
    let mut block = vec![0u8; PAGING_BLOCK_SIZE as usize];
    let mut put = |at: u32, entry: u64| {
        block[at as usize..at as usize + 8].copy_from_slice(&entry.to_le_bytes());
    };
    let xd = if nx { PTE_XD } else { 0 };
    let table = |addr: u32| u64::from(addr) | PTE_FLAGS;
    let large = |chunk: u32| u64::from(chunk) << LARGE_PAGE_SHIFT | PDE_LARGE;
    for i in 0..PDPT_FIRST_ENTRY {
        let entry = if i == TLM_CHUNK {
            table(base + TABLE_OFFSET)
        } else if i < EXE_CHUNKS {
            table(PROCESS_TABLES + i * PAGE_SIZE)
        } else if i == STUB_CHUNK {
            large(i) | PDE_READ_ONLY
        } else if i == TABLES_CHUNK || EXTRA_CHUNKS.contains(&i) || ARENA_CHUNKS.contains(&i) {
            large(i) | PTE_SUPERVISOR | xd
        } else if (ALLOC_FIRST_CHUNK..ALLOC_FIRST_CHUNK + ALLOC_CHUNKS).contains(&i) {
            table(ALLOC_TABLES + (i - ALLOC_FIRST_CHUNK) * PAGE_SIZE)
        } else if (SHARED_FIRST_CHUNK..SHARED_FIRST_CHUNK + SHARED_CHUNKS).contains(&i) {
            table(SHARED_TABLES + (i - SHARED_FIRST_CHUNK) * PAGE_SIZE)
        } else {
            large(i) | PTE_FLAGS | xd
        };
        put(DIRECTORY_OFFSET + ENTRY_BYTES * i, entry);
    }
    for j in 0..ENTRIES {
        let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
        let target = match j {
            TLM_ENTRY => base + TLM_OFFSET,
            RING0_ENTRY => base + RING0_OFFSET,
            _ => page,
        };
        let flags = if supervisor_only(page) { PTE_SUPERVISOR } else { PTE_FLAGS };
        let xd = if ring0_code(page) { 0 } else { xd };
        if j != FS_ENTRY {
            put(TABLE_OFFSET + ENTRY_BYTES * j, u64::from(target) | flags | xd);
        } else if tib != 0 {
            put(TABLE_OFFSET + ENTRY_BYTES * j, u64::from(tib & !(PAGE_SIZE - 1)) | PTE_FLAGS | xd);
        }
    }
    let mut put32 = |at: u32, v: u32| block[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    put32(RING0_OFFSET + TSS_ESP0_OFFSET, RING0_STACK_TOP);
    put32(RING0_OFFSET + TSS_SS0_OFFSET, u32::from(KERNEL_DATA_SEL));
    let iomap = (RING0_OFFSET + TSS_IOMAP_OFFSET) as usize;
    block[iomap..iomap + 2].copy_from_slice(&(TSS_SIZE as u16).to_le_bytes());
    block[PDPT_OFFSET as usize..(PDPT_OFFSET + PDPT_SIZE) as usize].copy_from_slice(&pdpt_of_block(base));
    block
}

/// The PDPT of the block at `base`: its own directory for the first
/// gigabyte, the process's three for the rest.
pub fn pdpt_of_block(base: u32) -> [u8; PDPT_SIZE as usize] {
    let mut pdpt = [0u8; PDPT_SIZE as usize];
    let dirs = [base + DIRECTORY_OFFSET, HIGH_DIRECTORIES, HIGH_DIRECTORIES + PAGE_SIZE, HIGH_DIRECTORIES + 2 * PAGE_SIZE];
    for (k, dir) in dirs.into_iter().enumerate() {
        pdpt[8 * k..8 * k + 8].copy_from_slice(&(u64::from(dir) | PDPTE_PRESENT).to_le_bytes());
    }
    pdpt
}

/// The CR3 value of the block at `base`: its PDPT, at the end of its
/// directory; None for an address that is no block.
pub fn cr3_of_block(base: u32) -> Option<u32> {
    valid_block(base).then_some(base + PDPT_OFFSET)
}

/// The block whose PDPT CR3 holds; None for a value that is no block's.
pub fn block_of_cr3(cr3: u64) -> Option<u32> {
    let pdpt = u32::try_from(cr3 & CR3_ADDRESS).ok()?;
    pdpt.checked_sub(PDPT_OFFSET).filter(|&base| valid_block(base))
}

/// The address of block `slot`: the first [`MAIN_BLOCKS`] after the
/// process tables, the rest in the extra 4 MB.
fn block_base(slot: u32) -> u32 {
    if slot < MAIN_BLOCKS {
        PAGING_BLOCKS_BASE + slot * PAGING_BLOCK_SIZE
    } else {
        PAGING_EXTRA_BASE + (slot - MAIN_BLOCKS) * PAGING_BLOCK_SIZE
    }
}

/// The slot of the block at `base`; None for an address that is no block.
fn block_slot(base: u32) -> Option<u32> {
    let within = |start: u32, count: u32| base.checked_sub(start)
        .filter(|off| off.is_multiple_of(PAGING_BLOCK_SIZE) && off / PAGING_BLOCK_SIZE < count)
        .map(|off| off / PAGING_BLOCK_SIZE);
    within(PAGING_BLOCKS_BASE, MAIN_BLOCKS).or_else(|| within(PAGING_EXTRA_BASE, EXTRA_BLOCKS).map(|i| MAIN_BLOCKS + i))
}

/// Whether `base` can be a paging block: one of the paging arena's
/// blocks (from [`PAGING_BLOCKS_BASE`] and [`PAGING_EXTRA_BASE`], every
/// [`PAGING_BLOCK_SIZE`] bytes).
pub fn valid_block(base: u32) -> bool {
    block_slot(base).is_some()
}

/// The thread-local page of the paging block CR3 `cr3` names; None for a
/// value that cannot be a block's.
pub fn tlm_page_of(cr3: u64) -> Option<u32> {
    Some(block_of_cr3(cr3)? + TLM_OFFSET)
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
            Some(base) if block_of_cr3(sregs.cr3) == Some(base) => PagingStatus::Own,
            _ => PagingStatus::ForeignCr3(sregs.cr3),
        }
    }

    /// Allocate and fill a thread's paging block; its CR3 value
    /// ([`cr3_of_block`]), or None when there is no memory.
    pub(crate) fn alloc_paging_block(&self, tib: u32) -> Option<u32> {
        self.ensure_process_tables();
        let base = self.shared.paging_arena.lock_or_recover().alloc()?;
        let block = paging_block(base, tib, nx_enforced());
        let cr3 = cr3_of_block(base);
        let written = cr3.is_some() && self.system_write_bytes(base, &block).is_some();
        if !written {
            self.shared.paging_arena.lock_or_recover().free(base);
            return None;
        }
        cr3
    }

    /// Write the process tables (ADR-0114) once, before the first thread's
    /// block or the first change of a page's state: the program area
    /// present and writable but the low 64 KB, the allocator's region not
    /// present; `vmem.rs` brings the allocations in and changes states.
    pub(crate) fn ensure_process_tables(&self) {
        if self.shared.process_tables_ready.swap(true, Ordering::AcqRel) {
            return;
        }
        // The program area's tables, the low 64 KB not present and the
        // rest not executable until `protect_objects` marks the code;
        // then the allocator's, not present until allocated (`vmem.rs`
        // brings the allocations in, those made before this too).
        let exe = (0..EXE_CHUNKS * ENTRIES).map(|i| i << PAGE_SHIFT)
            .map(|page| if page < LOW_GUARD { 0 } else { process_pte(page, PageAccess::ReadWrite) });
        let alloc = (0..ALLOC_CHUNKS * ENTRIES).map(|i| u64::from(DYNAMIC_ALLOC_BASE + (i << PAGE_SHIFT)));
        // The shared arena's, not present until a block held is touched
        // (`shared_arena.rs`).
        let shared = (0..SHARED_CHUNKS * ENTRIES).map(|i| u64::from(SHARED_ARENA_BASE + (i << PAGE_SHIFT)));
        // The 1-4 GB, one-to-one large pages, not executable.
        let xd = if nx_enforced() { PTE_XD } else { 0 };
        let high = (0..HIGH_DIRECTORY_COUNT * ENTRIES)
            .map(|i| (u64::from(ENTRIES + i) << LARGE_PAGE_SHIFT) | PTE_FLAGS | PDE_LARGE | xd);
        let tables: Vec<u8> = exe.chain(alloc).chain(shared).chain(high).flat_map(u64::to_le_bytes).collect();
        if self.system_write_bytes(PROCESS_TABLES, &tables).is_none() {
            warn!("the process page tables could not be written");
        }
    }

    /// Give back a thread's paging block (by the CR3 value
    /// [`Self::alloc_paging_block`] gave).
    pub(crate) fn free_paging_block(&self, cr3: u32) {
        let freed = block_of_cr3(u64::from(cr3)).is_some_and(|base| self.shared.paging_arena.lock_or_recover().free(base));
        if !freed {
            warn!("paging block of CR3 {cr3:#x} freed twice or never allocated");
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

    fn entry(block: &[u8], at: u32) -> u64 {
        u64::from_le_bytes(block[at as usize..at as usize + 8].try_into().unwrap())
    }
    /// Directory entry `i` and table entry `j` of a block.
    fn pde(block: &[u8], i: u32) -> u64 { entry(block, DIRECTORY_OFFSET + ENTRY_BYTES * i) }
    fn pte(block: &[u8], j: u32) -> u64 { entry(block, TABLE_OFFSET + ENTRY_BYTES * j) }
    const ADDRESS: u64 = 0x000F_FFFF_FFFF_F000;

    /// Every address maps to itself, except the thread-local page.
    #[test]
    fn the_tables_map_everything_one_to_one_but_the_thread_local_page() {
        const BASE: u32 = PAGING_BLOCKS_BASE + 3 * PAGING_BLOCK_SIZE;
        let b = paging_block(BASE, TIB_BASE, true);
        assert_eq!(b.len(), PAGING_BLOCK_SIZE as usize);
        let chunk = TLM_CHUNK << LARGE_PAGE_SHIFT;
        assert_eq!(chunk, SYSTEM_AREA_BASE & !0x1F_FFFF, "the system area's 2 MB");
        assert_eq!(pde(&b, TLM_CHUNK), u64::from(BASE + TABLE_OFFSET) | PTE_FLAGS, "those 2 MB through the table");
        for i in 0..EXE_CHUNKS {
            assert_eq!(pde(&b, i), u64::from(PROCESS_TABLES + i * PAGE_SIZE) | PTE_FLAGS, "program area chunk {i}: a process table");
        }
        for i in EXTRA_CHUNKS.chain(ARENA_CHUNKS).chain([TABLES_CHUNK]) {
            assert_eq!(pde(&b, i), u64::from(i) << LARGE_PAGE_SHIFT | PTE_SUPERVISOR | PDE_LARGE | PTE_XD,
                       "chunk {i}: Warpine's tables and blocks, supervisor-only (ADR-0115, 0131)");
        }
        let last = PDPT_FIRST_ENTRY - 1;
        assert_eq!(pde(&b, last), (u64::from(last) << LARGE_PAGE_SHIFT) | PTE_FLAGS | PDE_LARGE | PTE_XD);
        assert_eq!(pde(&b, PDPT_FIRST_ENTRY), u64::from(BASE) | PDPTE_PRESENT, "then the PDPT: supervisor-only, read-only as entries");
        assert_eq!(pte(&b, 0), u64::from(chunk) | PTE_SUPERVISOR | PTE_XD, "the GDT's first page: supervisor-only");
        let tib = (TIB_BASE >> PAGE_SHIFT) & (ENTRIES - 1);
        assert_eq!(pte(&b, tib), u64::from(TIB_BASE) | PTE_FLAGS | PTE_XD, "the TIB");
        assert_eq!(pte(&b, TLM_ENTRY), u64::from(BASE + TLM_OFFSET) | PTE_FLAGS | PTE_XD);
        assert_eq!(pte(&b, FS_ENTRY), u64::from(TIB_BASE) | PTE_FLAGS | PTE_XD, "the FS window");
        assert_eq!(pte(&paging_block(BASE, 0, true), FS_ENTRY), 0, "no TIB: no window");
        assert_eq!(pte(&b, ENTRIES - 1), u64::from(chunk + 0x001F_F000) | PTE_SUPERVISOR | PTE_XD, "the arena's last table page");
        assert!(b[TLM_OFFSET as usize..RING0_OFFSET as usize].iter().all(|&x| x == 0), "a zeroed thread-local page");
        assert_eq!(pte(&b, RING0_ENTRY), u64::from(BASE + RING0_OFFSET) | PTE_SUPERVISOR | PTE_XD, "the ring-0 window");
        assert_eq!(pde(&b, STUB_CHUNK), (u64::from(STUB_CHUNK) << LARGE_PAGE_SHIFT) | PDE_READ_ONLY | PDE_LARGE,
                   "the thunks: read-only, executable");
        assert_eq!(tlm_page_of(cr3_of_block(BASE).unwrap() as u64), Some(BASE + TLM_OFFSET));
    }

    /// No entry maps a page anywhere but to itself, except the
    /// thread-local page, which maps into the block itself, and the FS
    /// window, which shows the thread's own TIB: paging gives the guest no
    /// view it did not have unpaged.
    #[test]
    fn no_entry_maps_a_page_elsewhere_than_itself_but_the_thread_local_page() {
        const BASE: u32 = PAGING_BLOCKS_BASE;
        let b = paging_block(BASE, TIB_BASE, true);
        let alloc = ALLOC_FIRST_CHUNK..ALLOC_FIRST_CHUNK + ALLOC_CHUNKS;
        for i in alloc.clone() {
            assert_eq!(pde(&b, i), u64::from(ALLOC_TABLES + (i - ALLOC_FIRST_CHUNK) * PAGE_SIZE) | PTE_FLAGS,
                       "allocator chunk {i}: through its process table");
        }
        let shared = SHARED_FIRST_CHUNK..SHARED_FIRST_CHUNK + SHARED_CHUNKS;
        for i in shared.clone() {
            assert_eq!(pde(&b, i), u64::from(SHARED_TABLES + (i - SHARED_FIRST_CHUNK) * PAGE_SIZE) | PTE_FLAGS,
                       "shared arena chunk {i}: through its process table");
        }
        for i in (EXE_CHUNKS..PDPT_FIRST_ENTRY).filter(|&i| i != TLM_CHUNK && !alloc.contains(&i) && !shared.contains(&i)) {
            let e = pde(&b, i);
            assert_eq!(e & ADDRESS, u64::from(i) << LARGE_PAGE_SHIFT, "directory entry {i}");
            let flags = if i == STUB_CHUNK { PDE_READ_ONLY }
                        else if i == TABLES_CHUNK || EXTRA_CHUNKS.contains(&i) || ARENA_CHUNKS.contains(&i) { PTE_SUPERVISOR | PTE_XD }
                        else { PTE_FLAGS | PTE_XD };
            assert_eq!(e & !ADDRESS, flags | PDE_LARGE, "directory entry {i}");
        }
        for j in 0..ENTRIES {
            let e = pte(&b, j);
            let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
            let flags = if supervisor_only(page) { PTE_SUPERVISOR } else { PTE_FLAGS };
            let xd = if ring0_code(page) { 0 } else { PTE_XD };
            assert_eq!(e & !ADDRESS, flags | xd, "table entry {j}");
            let target = (e & ADDRESS) as u32;
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

    /// Execute permission (ADR-0131): only the thunks and the IDT's ring-0
    /// stubs among the large pages and the system area; with execute
    /// protection off, every entry is executable.
    #[test]
    fn only_code_is_executable() {
        const BASE: u32 = PAGING_BLOCKS_BASE + PAGING_BLOCK_SIZE;
        let b = paging_block(BASE, TIB_BASE, true);
        let exec = |e: u64| e & PTE_PRESENT != 0 && e & PTE_XD == 0;
        let executable: Vec<u32> = (0..ENTRIES).filter(|&j| exec(pte(&b, j)))
            .map(|j| (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT)).collect();
        assert!(!executable.is_empty() && executable.iter().all(|&p| ring0_code(p)), "{executable:x?}");
        assert!(executable.contains(&(IDT_HANDLER_BASE & !(PAGE_SIZE - 1))), "the IDT's stubs run");
        let large: Vec<u32> = (0..ENTRIES).filter(|&i| pde(&b, i) & PDE_LARGE != 0 && exec(pde(&b, i))).collect();
        assert_eq!(large, vec![STUB_CHUNK], "of the large pages only the thunks");
        let off = paging_block(BASE, TIB_BASE, false);
        assert!((0..ENTRIES).all(|i| pde(&off, i) & PTE_XD == 0 && pte(&off, i) & PTE_XD == 0), "NX off: nothing is execute-disabled");
        // The process-table entries follow the access.
        let page = DYNAMIC_ALLOC_BASE + 0x5000;
        assert_eq!(process_pte_nx(page, PageAccess::ReadWrite, true), u64::from(page) | PTE_FLAGS | PTE_XD);
        assert_eq!(process_pte_nx(page, PageAccess::ReadWriteExec, true), u64::from(page) | PTE_FLAGS);
        assert_eq!(process_pte_nx(page, PageAccess::ReadExec, true), u64::from(page) | PTE_PRESENT | PTE_USER | PTE_ACCESSED);
        assert_eq!(process_pte_nx(page, PageAccess::Read, false), u64::from(page) | PTE_PRESENT | PTE_USER | PTE_ACCESSED, "NX off");
        assert_eq!(process_pte_nx(page, PageAccess::None, true), u64::from(page), "not present, no XD");
        for a in [PageAccess::None, PageAccess::Read, PageAccess::ReadWrite, PageAccess::ReadExec, PageAccess::ReadWriteExec] {
            assert_eq!(pte_access(process_pte_nx(page, a, true)), a, "{a:?} round trip");
        }
    }

    /// PageAccess's permissions and which changes lose one (a TLB flush).
    #[test]
    fn page_access_permissions() {
        use PageAccess::*;
        assert_eq!(PageAccess::of(false, false, false), None);
        assert_eq!(PageAccess::of(true, true, true), ReadWriteExec);
        assert_eq!(PageAccess::of(false, false, true), ReadExec, "execute alone reads too");
        assert_eq!(ReadWrite.with_exec(true), ReadWriteExec);
        assert_eq!(None.with_exec(true), None, "a page not present stays so");
        assert!(ReadWriteExec.loses_to(ReadWrite) && ReadExec.loses_to(Read), "losing execute");
        assert!(ReadWrite.loses_to(ReadExec), "losing write");
        assert!(!Read.loses_to(ReadExec) && !ReadWrite.loses_to(ReadWriteExec) && !None.loses_to(Read));
        assert!(nx_setting(Option::None) && nx_setting(Some("on")) && nx_setting(Some("")));
        assert!(!nx_setting(Some("off")) && !nx_setting(Some("OFF")) && !nx_setting(Some("0")) && !nx_setting(Some("no")));
    }

    /// The ring-0 page starts with a TSS whose ring-0 stack is the rest of
    /// that page, on the ring-0 data selector, with no I/O bitmap; the PDPT
    /// CR3 names lies apart, in the PDPT table: the block's directory and
    /// the three shared ones.
    #[test]
    fn the_ring0_page_holds_the_threads_tss_and_stack() {
        const BASE: u32 = PAGING_BLOCKS_BASE + 5 * PAGING_BLOCK_SIZE;
        let b = paging_block(BASE, TIB_BASE, true);
        let word = |off: u32| u32::from_le_bytes(b[(RING0_OFFSET + off) as usize..(RING0_OFFSET + off + 4) as usize].try_into().unwrap());
        assert_eq!(word(TSS_ESP0_OFFSET), RING0_WINDOW + PAGE_SIZE, "ESP0: the top of the ring-0 page");
        assert_eq!(word(TSS_SS0_OFFSET), u32::from(KERNEL_DATA_SEL), "SS0: ring-0 data");
        assert_eq!(word(TSS_IOMAP_OFFSET - 2) >> 16, TSS_SIZE, "no I/O permission bitmap");
        let p = pdpt_of_block(BASE);
        let pdpt: Vec<u64> = (0..4).map(|k| entry(&p, 8 * k)).collect();
        assert_eq!(pdpt, vec![u64::from(BASE) | PDPTE_PRESENT, u64::from(HIGH_DIRECTORIES) | PDPTE_PRESENT,
                              u64::from(HIGH_DIRECTORIES + PAGE_SIZE) | PDPTE_PRESENT, u64::from(HIGH_DIRECTORIES + 2 * PAGE_SIZE) | PDPTE_PRESENT]);
        assert_eq!(cr3_of_block(BASE), Some(BASE + PAGE_SIZE - PDPT_SIZE), "the PDPT ends the directory");
        assert_eq!(&b[PDPT_OFFSET as usize..(PDPT_OFFSET + PDPT_SIZE) as usize], &p[..], "written in the block");
        assert!(supervisor_only(BASE), "in the supervisor-only arena");
        assert_eq!(block_of_cr3(cr3_of_block(BASE).unwrap() as u64), Some(BASE));
        assert_eq!(ring0_page_of_tlm(BASE + TLM_OFFSET), Some(BASE + RING0_OFFSET));
        assert_eq!(ring0_page_of_tlm(0), None);
        assert_eq!(KERNEL_DATA_SEL & 3, 0, "SS0's RPL is 0");
        let fields = [TSS_ESP0_OFFSET, TSS_SS0_OFFSET, TSS_IOMAP_OFFSET - 2];
        for off in (0..PAGE_SIZE).step_by(4).filter(|o| !fields.contains(o)) {
            assert_eq!(word(off), 0, "ring-0 page offset {off:#x}");
        }
    }

    /// Only one of the paging arena's blocks is accepted, whatever a CR3
    /// value holds; nothing overflows.
    #[test]
    fn only_the_arenas_blocks_are_accepted() {
        let last_main = PAGING_BLOCKS_BASE + (MAIN_BLOCKS - 1) * PAGING_BLOCK_SIZE;
        let last = PAGING_EXTRA_BASE + (EXTRA_BLOCKS - 1) * PAGING_BLOCK_SIZE;
        assert!(valid_block(PAGING_BLOCKS_BASE) && valid_block(last_main));
        assert!(valid_block(PAGING_EXTRA_BASE) && valid_block(last));
        assert!(last_main + PAGING_BLOCK_SIZE <= PAGING_ARENA_END && last + PAGING_BLOCK_SIZE <= PAGING_EXTRA_END);
        // Every slot's block, and back.
        assert_eq!((0..PAGING_BLOCKS).map(|i| block_slot(block_base(i))).collect::<Vec<_>>(),
                   (0..PAGING_BLOCKS).map(Some).collect::<Vec<_>>());
        assert_eq!((block_base(MAIN_BLOCKS - 1), block_base(MAIN_BLOCKS)), (last_main, PAGING_EXTRA_BASE));
        for bad in [0, TLM_PAGE, PROCESS_TABLES, PAGING_BLOCKS_BASE - PAGE_SIZE,
                    PAGING_BLOCKS_BASE + PAGE_SIZE, PAGING_BLOCKS_BASE + 1,
                    last_main + PAGING_BLOCK_SIZE, PAGING_ARENA_END, DYNAMIC_ALLOC_BASE,
                    PAGING_EXTRA_BASE - PAGING_BLOCK_SIZE, PAGING_EXTRA_BASE + PAGE_SIZE, PAGING_EXTRA_END,
                    MAGIC_API_BASE as u32, !0xFFF, u32::MAX] {
            assert!(!valid_block(bad), "{bad:#x}");
            assert_eq!(block_of_cr3(u64::from(bad)), None, "{bad:#x}");
        }
        let cr3 = u64::from(cr3_of_block(PAGING_BLOCKS_BASE).unwrap());
        assert_eq!(tlm_page_of(cr3), Some(PAGING_BLOCKS_BASE + TLM_OFFSET));
        assert_eq!(tlm_page_of(1u64 << 32 | cr3), None, "CR3 above 4 GB");
        assert_eq!(tlm_page_of(cr3 | CR3_PWT), Some(PAGING_BLOCKS_BASE + TLM_OFFSET),
                   "CR3's ignored bit (a TLB flush) is not part of the address");
        assert_eq!(tlm_page_of(u64::from(PAGING_BLOCKS_BASE)), None, "a block's base is no CR3");
        assert_eq!(tlm_page_of(u64::MAX), None);
        assert_eq!(block_of_tlm_page(0), None);
        assert_eq!(block_of_tlm_page(u32::MAX), None);
        assert_eq!(block_of_tlm_page(PAGING_BLOCKS_BASE + TLM_OFFSET), Some(PAGING_BLOCKS_BASE));
    }

    /// The process tables (ADR-0114) map the program area (but the low
    /// 64 KB) present and writable, and the allocator's region page by page,
    /// present while allocated, and lie in their own supervisor-only large
    /// page (ADR-0131); the entry helpers address and fill them.
    #[test]
    fn the_process_tables_map_the_allocators_region() {
        assert_eq!(ALLOC_CHUNKS * (1 << LARGE_PAGE_SHIFT), SHARED_ARENA_BASE - DYNAMIC_ALLOC_BASE);
        assert!(supervisor_only(PROCESS_TABLES) && supervisor_only(PROCESS_TABLES + PROCESS_TABLES_SIZE - PAGE_SIZE));
        assert_eq!(process_pte_at(0), Some(PROCESS_TABLES), "the program area first");
        assert_eq!(process_pte_at(EXE_AREA_END - 1), Some(ALLOC_TABLES - 8));
        assert_eq!(process_pte_at(EXE_AREA_END), None, "the API thunks keep their large page");
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE), Some(ALLOC_TABLES));
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE + 0x234), Some(ALLOC_TABLES));
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE + 0x1234), Some(ALLOC_TABLES + 8));
        assert_eq!(process_pte_at(SHARED_ARENA_BASE - 1), Some(SHARED_TABLES - 8));
        assert_eq!(process_pte_at(SHARED_ARENA_BASE), Some(SHARED_TABLES), "the shared arena's tables follow");
        assert_eq!(process_pte_at(SHARED_ARENA_BASE + SHARED_ARENA_SIZE - 1), Some(HIGH_DIRECTORIES - 8));
        assert_eq!(process_pte_at(SHARED_ARENA_BASE + SHARED_ARENA_SIZE), None);
        assert_eq!(process_pte_at(DYNAMIC_ALLOC_BASE - 1), None);
        let loader = super::super::Loader::new_mock();
        loader.ensure_process_tables();
        for addr in [LOW_GUARD, EXE_AREA_START + 0x5000, EXE_AREA_END - PAGE_SIZE] {
            let at = process_pte_at(addr).unwrap();
            assert_eq!(loader.guest_read::<u64>(at), Some(u64::from(addr) | PTE_FLAGS | PTE_XD),
                       "{addr:#x}: the program area present, not executable until its code is marked");
        }
        for addr in [DYNAMIC_ALLOC_BASE, DYNAMIC_ALLOC_BASE + 0x5000, SHARED_ARENA_BASE - PAGE_SIZE] {
            let at = process_pte_at(addr).unwrap();
            assert_eq!(loader.guest_read::<u64>(at), Some(u64::from(addr)), "{addr:#x}: the allocator's region not present until allocated");
        }
        // The 1-4 GB: one-to-one large pages, not executable.
        assert_eq!(loader.guest_read::<u64>(HIGH_DIRECTORIES), Some((1u64 << 30) | PTE_FLAGS | PDE_LARGE | PTE_XD));
        assert_eq!(loader.guest_read::<u64>(HIGH_DIRECTORIES + 3 * PAGE_SIZE - 8),
                   Some((0xFFE0_0000u64) | PTE_FLAGS | PDE_LARGE | PTE_XD), "the last 2 MB below 4 GB");
        // An allocation comes in with the next sync, its free goes out.
        let block = loader.shared.mem_mgr.lock().unwrap().alloc(2 * PAGE_SIZE).unwrap();
        loader.sync_allocations();
        let entry = |a: u32| loader.guest_read::<u64>(process_pte_at(a).unwrap()).unwrap();
        assert_eq!((entry(block), entry(block + PAGE_SIZE)),
                   (u64::from(block) | PTE_FLAGS | PTE_XD, u64::from(block + PAGE_SIZE) | PTE_FLAGS | PTE_XD));
        assert!(loader.shared.mem_mgr.lock().unwrap().free(block));
        loader.sync_allocations();
        assert_eq!(entry(block) & PTE_PRESENT, 0, "freed: not present");
        for addr in (0..LOW_GUARD).step_by(PAGE_SIZE as usize) {
            assert_eq!(loader.guest_read::<u64>(process_pte_at(addr).unwrap()).map(|e| e & PTE_PRESENT), Some(0),
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
                     PAGING_ARENA_BASE, PAGING_ARENA_END - PAGE_SIZE, PROCESS_TABLES, PAGING_EXTRA_BASE] {
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
        let cr3 = loader.alloc_paging_block(TIB_BASE).expect("a paging block");
        let paged = |cr3: u64| super::super::vm_backend::GuestSregs { cr0: CR0_PG | 1, cr3, ..Default::default() };
        super::super::guest_mem::set_thread_tlm_backing(tlm_page_of(cr3 as u64).unwrap());
        assert_eq!(loader.paging_status(&paged(cr3 as u64)), PagingStatus::Own);
        assert_eq!(loader.paging_status(&paged(cr3 as u64 | CR3_PWT)), PagingStatus::Own, "a TLB flush's CR3");
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
        let base = block_of_cr3(loader.alloc_paging_block(TIB_BASE).expect("a paging block").into()).unwrap();
        for addr in [GDT_BASE, IDT_BASE, IDT_HANDLER_BASE, TIB_BASE - 4, PAGING_ARENA_BASE, base, base + TLM_OFFSET, PAGING_ARENA_END - 4,
                     PAGING_EXTRA_BASE, PAGING_EXTRA_END - 4] {
            assert!(protected(addr, 4), "{addr:#x}");
            assert_eq!(loader.guest_write::<u32>(addr, 0x5151), None, "{addr:#x}: refused");
            assert!(loader.guest_write_bytes(addr, &[1, 2, 3, 4]).is_none(), "{addr:#x}");
            assert!(loader.guest_slice_mut(addr, 4).is_none(), "{addr:#x}");
            assert!(loader.guest_read::<u32>(addr).is_some(), "{addr:#x}: reads stay possible");
        }
        assert!(protected(TIB_BASE - 2, 4), "a write that straddles the edge");
        assert!(supervisor_only(PAGING_EXTRA_BASE) && supervisor_only(PAGING_EXTRA_END - PAGE_SIZE));
        assert!(protected(PAGING_EXTRA_BASE - 2, 4), "a write into the extra blocks from below");
        for addr in [TIB_BASE, PIB_BASE, TLM_PAGE, FS_WINDOW, PAGING_ARENA_END, DYNAMIC_ALLOC_BASE, PAGING_EXTRA_BASE - 4] {
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
        let cr3 = loader.alloc_paging_block(TIB_BASE).expect("a paging block");
        let own = tlm_page_of(cr3 as u64).unwrap();
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
