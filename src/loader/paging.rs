// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Guest paging for thread-local memory (ADR-0088).
//!
//! 32-bit guest threads run with 32-bit paging (`CR0.PG`, `CR4.PSE`):
//! - each thread has its own block of three pages: a page directory, one
//!   page table and its thread-local page;
//! - the directory maps the whole 4 GB linear space one-to-one with 4 MB
//!   pages, except the 4 MB holding `TLM_PAGE` (in the system area,
//!   ADR-0094), which go through the page table;
//! - the page table maps those 4 MB one-to-one too, except `TLM_PAGE`,
//!   which leads to the thread's own page.
//!
//! So every address means what it meant unpaged, and `TLM_PAGE` holds
//! each thread's own words. EPT / NPT still bound the guest to its memory:
//! tables a guest damages can only map memory it could already reach.
//! The host side translates `TLM_PAGE` the same way for the thread a vCPU
//! thread runs (`guest_mem::set_thread_tlm_backing`).
//!
//! Security (the guest runs at ring 0 and may rewrite its own tables or
//! load CR3 / CR0 itself):
//! - the host never walks the guest's tables: `guest_mem` translates only
//!   `TLM_PAGE`, to the page Warpine itself chose for the vCPU thread when
//!   the thread started; every access stays bounds-checked;
//! - a block address is used only when it lies, whole and page-aligned, in
//!   the allocator's region ([`valid_block`]), with checked arithmetic;
//! - OS/2 programs cannot touch paging (`mov cr3` is privileged, the
//!   tables are not theirs), so a guest that turns paging off, loads its
//!   own CR3 or changes its tables ends as a trap (`TC_TRAP`): a watch
//!   thread kicks the paged vCPUs every [`PAGING_WATCH_INTERVAL`] and each
//!   kicked exit looks ([`super::Loader::paging_violation`]). The kick also
//!   ends a vCPU spinning in KVM after such a change, which never exits on
//!   its own;
//! - damaged tables hurt only the guest (EPT / NPT bound it): a fault ends
//!   the process; before thread 1 runs its exit list after a fault, Warpine
//!   rewrites the thread's tables and CR3 ([`super::Loader::repair_paging_block`]);
//! - crash reports say whether paging is on, whether CR3 is the thread's
//!   own block, and whether its tables are intact ([`PagingStatus`]).
//!
//! `DosAllocThreadLocalMemory` / `DosFreeThreadLocalMemory` (Control
//! Program Programming Reference) hand out words of the 32-DWORD
//! thread-local area at the start of that page, 1 to 8 per call; the
//! same words are reserved in every thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::constants::*;
use super::mutex_ext::MutexExt;
use log::{debug, warn};

/// How often the paged vCPUs are kicked to look at their paging.
pub const PAGING_WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Kick every vCPU of the process every [`PAGING_WATCH_INTERVAL`] until it
/// ends, so that each looks at its paging (see the module documentation).
pub fn start_paging_watch(shared: Arc<super::SharedState>) {
    let spawned = std::thread::Builder::new().name("os2-paging-watch".into()).spawn(move || {
        while !shared.exit_requested.load(Ordering::Relaxed) {
            std::thread::sleep(PAGING_WATCH_INTERVAL);
            shared.threads.table.lock_or_recover().kick_all_but(0);
        }
    });
    if let Err(e) = spawned {
        warn!("no paging watch thread ({e}): a guest changing its paging is not caught");
    }
}

/// Pages in a thread's paging block: directory, table, thread-local page.
pub const PAGING_BLOCK_PAGES: u32 = 3;
/// Bytes of a thread's paging block.
pub const PAGING_BLOCK_SIZE: u32 = PAGING_BLOCK_PAGES * PAGE_SIZE;
/// Offsets of the parts in the block.
const TABLE_OFFSET: u32 = PAGE_SIZE;
const TLM_OFFSET: u32 = 2 * PAGE_SIZE;

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
const ENTRIES: u32 = 1024;
const LARGE_PAGE_SHIFT: u32 = 22;
/// The 4 MB block (directory entry) holding `TLM_PAGE`: the one mapped
/// through the page table.
const TLM_CHUNK: u32 = TLM_PAGE >> LARGE_PAGE_SHIFT;
/// `TLM_PAGE`'s entry in the page table.
const TLM_ENTRY: u32 = (TLM_PAGE >> PAGE_SHIFT) & (ENTRIES - 1);
const PAGE_SHIFT: u32 = 12;

/// Control-register bits.
pub const CR0_PG: u64 = 1 << 31;
pub const CR4_PSE: u64 = 1 << 4;

/// DWORDs in the thread-local area, and most per call.
pub const TLM_WORDS: u32 = 32;
pub const TLM_MAX_PER_CALL: u32 = 8;

/// The paging block's contents for a block at `base`: the directory, the
/// table and a zeroed thread-local page.
pub fn paging_block(base: u32) -> Vec<u8> {
    let mut block = vec![0u8; PAGING_BLOCK_SIZE as usize];
    let mut put = |at: u32, entry: u32| {
        block[at as usize..at as usize + 4].copy_from_slice(&entry.to_le_bytes());
    };
    for i in 0..ENTRIES {
        if i == TLM_CHUNK {
            put(4 * i, (base + TABLE_OFFSET) | PTE_FLAGS);
        } else {
            put(4 * i, (i << LARGE_PAGE_SHIFT) | PTE_FLAGS | PDE_LARGE);
        }
    }
    for j in 0..ENTRIES {
        let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
        let target = if j == TLM_ENTRY { base + TLM_OFFSET } else { page };
        put(TABLE_OFFSET + 4 * j, target | PTE_FLAGS);
    }
    block
}

/// Bytes of a block Warpine rewrites to repair it: the directory and the
/// table (the thread-local page keeps its words).
const TABLES_SIZE: u32 = TLM_OFFSET;

/// Whether `base` can be a paging block: page-aligned and wholly inside
/// the allocator's region (`DYNAMIC_ALLOC_BASE`..`SHARED_ARENA_BASE`).
pub fn valid_block(base: u32) -> bool {
    base.is_multiple_of(PAGE_SIZE) && base >= DYNAMIC_ALLOC_BASE
        && base.checked_add(PAGING_BLOCK_SIZE).is_some_and(|end| end <= SHARED_ARENA_BASE)
}

/// The thread-local page of the paging block at `cr3`; None for an
/// address that cannot be a block.
pub fn tlm_page_of(cr3: u64) -> Option<u32> {
    let base = u32::try_from(cr3).ok().filter(|&b| valid_block(b))?;
    Some(base + TLM_OFFSET)
}

/// The paging block whose thread-local page is `page`.
pub fn block_of_tlm_page(page: u32) -> Option<u32> {
    page.checked_sub(TLM_OFFSET).filter(|&b| valid_block(b))
}

/// The paging state of a vCPU, for crash reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingStatus {
    /// CR0.PG is off (16-bit programs, or a guest that turned it off).
    Off,
    /// CR3 is the thread's own block and its tables are as Warpine wrote them.
    Intact,
    /// The thread's own tables were changed (whatever CR3 holds now).
    Damaged,
    /// CR3 is not the block Warpine gave the thread (the guest loaded it).
    ForeignCr3(u64),
}

impl std::fmt::Display for PagingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PagingStatus::Off => write!(f, "off"),
            PagingStatus::Intact => write!(f, "on, the thread's tables intact"),
            PagingStatus::Damaged => write!(f, "the thread's tables DAMAGED by the guest"),
            PagingStatus::ForeignCr3(cr3) => write!(f, "on, CR3 0x{cr3:08X} is not the thread's block (loaded by the guest)"),
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
    /// Whether the directory and table of the block at `base` are as
    /// Warpine wrote them, but for the accessed and dirty bits (which the
    /// processor may set, though Warpine sets them already).
    pub(crate) fn paging_block_intact(&self, base: u32) -> bool {
        const AD: u32 = PTE_ACCESSED | PTE_DIRTY;
        let entries = |b: &[u8]| b.as_chunks::<4>().0.iter()
            .map(|e| u32::from_le_bytes(*e) | AD).collect::<Vec<u32>>();
        valid_block(base) && self.guest_read_bytes(base, TABLES_SIZE as usize)
            .is_some_and(|now| entries(&now) == entries(&paging_block(base)[..TABLES_SIZE as usize]))
    }

    /// Rewrite the directory and table of the block at `base` (its
    /// thread-local words stay); false for an address that is no block.
    pub(crate) fn repair_paging_block(&self, base: u32) -> bool {
        valid_block(base) && self.guest_write_bytes(base, &paging_block(base)[..TABLES_SIZE as usize]).is_some()
    }

    /// The paging state of a vCPU with `sregs` on this host thread (whose
    /// block `guest_mem` knows from `run_vcpu`).
    pub(crate) fn paging_status(&self, sregs: &super::vm_backend::GuestSregs) -> PagingStatus {
        // The thread's own tables first: a triple fault resets the vCPU's
        // registers, but damage the guest did is still in memory.
        let own = block_of_tlm_page(super::guest_mem::thread_tlm_backing());
        if own.is_some_and(|base| !self.paging_block_intact(base)) {
            return PagingStatus::Damaged;
        }
        if sregs.cr0 & CR0_PG == 0 {
            return PagingStatus::Off;
        }
        match own {
            Some(base) if sregs.cr3 == base as u64 => PagingStatus::Intact,
            _ => PagingStatus::ForeignCr3(sregs.cr3),
        }
    }

    /// What is wrong with the paging of `vcpu`, a thread this host thread
    /// runs, when the guest changed it: its tables changed, paging off, or
    /// a CR3 that is not its block. None when it is as Warpine set it up,
    /// or the thread runs unpaged anyway.
    pub(crate) fn paging_violation(&self, vcpu: &dyn super::vm_backend::VcpuBackend) -> Option<PagingStatus> {
        block_of_tlm_page(super::guest_mem::thread_tlm_backing())?;
        let sregs = vcpu.get_sregs().ok()?;
        match self.paging_status(&sregs) {
            PagingStatus::Intact => None,
            other => Some(other),
        }
    }

    /// Allocate and fill a thread's paging block; its address (the CR3
    /// value), or None when there is no memory.
    pub(crate) fn alloc_paging_block(&self) -> Option<u32> {
        let base = self.shared.mem_mgr.lock_or_recover().alloc(PAGING_BLOCK_SIZE)?;
        if !valid_block(base) || self.guest_write_bytes(base, &paging_block(base)).is_none() {
            self.shared.mem_mgr.lock_or_recover().free(base);
            return None;
        }
        Some(base)
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
        const BASE: u32 = 0x0200_3000;
        let b = paging_block(BASE);
        assert_eq!(b.len(), PAGING_BLOCK_SIZE as usize);
        let chunk = TLM_CHUNK << LARGE_PAGE_SHIFT;
        assert_eq!(chunk, SYSTEM_AREA_BASE & !0x3F_FFFF, "the system area's 4 MB");
        assert_eq!(entry(&b, 4 * TLM_CHUNK), (BASE + TABLE_OFFSET) | PTE_FLAGS, "those 4 MB through the table");
        assert_eq!(entry(&b, 0), PTE_FLAGS | PDE_LARGE, "the first 4 MB, one large page");
        assert_eq!(entry(&b, 4), 0x0040_0000 | PTE_FLAGS | PDE_LARGE);
        assert_eq!(entry(&b, 4 * 1023), 0xFFC0_0000 | PTE_FLAGS | PDE_LARGE);
        assert_eq!(entry(&b, TABLE_OFFSET), chunk | PTE_FLAGS, "the block's first page");
        let tib = (TIB_BASE >> PAGE_SHIFT) & (ENTRIES - 1);
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * tib), TIB_BASE | PTE_FLAGS, "the TIB");
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * TLM_ENTRY), (BASE + TLM_OFFSET) | PTE_FLAGS);
        assert_eq!(entry(&b, TABLE_OFFSET + 4 * 1023), (chunk + 0x003F_F000) | PTE_FLAGS);
        assert!(b[TLM_OFFSET as usize..].iter().all(|&x| x == 0), "a zeroed thread-local page");
        assert_eq!(tlm_page_of(BASE as u64), Some(BASE + TLM_OFFSET));
    }

    /// No entry maps a page anywhere but to itself, except the
    /// thread-local page, which maps into the block itself: paging gives
    /// the guest no view it did not have unpaged.
    #[test]
    fn no_entry_maps_a_page_elsewhere_than_itself_but_the_thread_local_page() {
        const BASE: u32 = 0x0300_0000;
        let b = paging_block(BASE);
        for i in (0..ENTRIES).filter(|&i| i != TLM_CHUNK) {
            let e = entry(&b, 4 * i);
            assert_eq!(e & !0x3F_FFFF, i << LARGE_PAGE_SHIFT, "directory entry {i}");
            assert_eq!(e & 0xFFF, PTE_FLAGS | PDE_LARGE);
        }
        for j in 0..ENTRIES {
            let e = entry(&b, TABLE_OFFSET + 4 * j);
            assert_eq!(e & 0xFFF, PTE_FLAGS, "table entry {j}");
            let target = e & !0xFFF;
            let page = (TLM_CHUNK << LARGE_PAGE_SHIFT) | (j << PAGE_SHIFT);
            if page == TLM_PAGE {
                assert_eq!(target, BASE + TLM_OFFSET);
                assert!(valid_block(target - TLM_OFFSET));
            } else {
                assert_eq!(target, page, "table entry {j}");
            }
        }
    }

    /// Only a page-aligned block wholly inside the allocator's region is
    /// accepted, whatever a CR3 value holds; nothing overflows.
    #[test]
    fn only_blocks_in_the_allocators_region_are_accepted() {
        assert!(valid_block(DYNAMIC_ALLOC_BASE));
        assert!(valid_block(SHARED_ARENA_BASE - PAGING_BLOCK_SIZE));
        for bad in [0, TLM_PAGE, DYNAMIC_ALLOC_BASE - PAGE_SIZE, DYNAMIC_ALLOC_BASE + 1,
                    SHARED_ARENA_BASE - PAGING_BLOCK_SIZE + PAGE_SIZE, SHARED_ARENA_BASE, !0xFFF, u32::MAX] {
            assert!(!valid_block(bad), "{bad:#x}");
            assert_eq!(tlm_page_of(bad as u64), None, "{bad:#x}");
        }
        assert_eq!(tlm_page_of(1u64 << 32 | DYNAMIC_ALLOC_BASE as u64), None, "CR3 above 4 GB");
        assert_eq!(tlm_page_of(u64::MAX), None);
        assert_eq!(block_of_tlm_page(0), None);
        assert_eq!(block_of_tlm_page(u32::MAX), None);
        assert_eq!(block_of_tlm_page(DYNAMIC_ALLOC_BASE + TLM_OFFSET), Some(DYNAMIC_ALLOC_BASE));
    }

    /// Tables the guest damaged are seen, reported and repaired; the
    /// thread-local words survive a repair.
    #[test]
    fn damaged_tables_are_seen_and_repaired() {
        let loader = super::super::Loader::new_mock();
        let base = loader.alloc_paging_block().expect("a paging block");
        let paged = |cr3: u64| super::super::vm_backend::GuestSregs { cr0: CR0_PG | 1, cr3, ..Default::default() };
        super::super::guest_mem::set_thread_tlm_backing(tlm_page_of(base as u64).unwrap());
        assert!(loader.paging_block_intact(base));
        assert_eq!(loader.paging_status(&paged(base as u64)), PagingStatus::Intact);
        assert_eq!(loader.paging_status(&Default::default()), PagingStatus::Off);
        assert_eq!(loader.paging_status(&paged(0x1234_5000)), PagingStatus::ForeignCr3(0x1234_5000));
        // The processor's accessed / dirty bits are no damage.
        let e = loader.guest_read::<u32>(base + 4 * 7).unwrap();
        loader.guest_write::<u32>(base + 4 * 7, e & !(PTE_ACCESSED | PTE_DIRTY)).unwrap();
        assert!(loader.paging_block_intact(base));
        // The guest zeroes a directory entry and remaps its table's TLM entry.
        loader.guest_write::<u32>(base + 4 * 5, 0).unwrap();
        loader.guest_write::<u32>(base + TABLE_OFFSET + 4 * TLM_ENTRY, 0x0010_0007).unwrap();
        loader.guest_write::<u32>(base + TLM_OFFSET, 0x5151).unwrap();
        assert!(!loader.paging_block_intact(base));
        assert_eq!(loader.paging_status(&paged(base as u64)), PagingStatus::Damaged);
        assert!(loader.repair_paging_block(base));
        assert!(loader.paging_block_intact(base));
        assert_eq!(loader.guest_read::<u32>(base + TLM_OFFSET), Some(0x5151), "the words stay");
        assert!(!loader.repair_paging_block(0x1000), "no block there");
        assert!(!loader.paging_block_intact(0x1000));
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
        let block = loader.alloc_paging_block().expect("a paging block");
        let own = tlm_page_of(block as u64).unwrap();
        loader.guest_write::<u32>(TLM_PAGE, 0x1111).unwrap();
        super::super::guest_mem::set_thread_tlm_backing(own);
        loader.guest_write::<u32>(TLM_PAGE + 4, 0x2222).unwrap();
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE), Some(0), "this thread's page");
        assert_eq!(loader.guest_read::<u32>(own + 4), Some(0x2222));
        // An access that crosses the page's end is not translated.
        assert_eq!(loader.guest_read::<u64>(TLM_PAGE + PAGE_SIZE - 4).map(|v| v as u32), Some(0));
        super::super::guest_mem::set_thread_tlm_backing(0);
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE), Some(0x1111));
        assert_eq!(loader.guest_read::<u32>(TLM_PAGE + 4), Some(0));
    }

    /// A guest that changed its paging is caught: tables, paging off, its
    /// own CR3; a thread running as set up, or unpaged from the start, is not.
    #[test]
    fn a_guest_changing_its_paging_is_caught() {
        use super::super::vm_backend::mock::MockVcpu;
        let loader = super::super::Loader::new_mock();
        let base = loader.alloc_paging_block().expect("a paging block");
        let mut vcpu = MockVcpu::new();
        loader.setup_vcpu_segments_32bit(&mut vcpu, TIB_BASE as u64, base);
        assert_eq!(loader.paging_violation(&vcpu), None, "no block known: an unpaged thread");
        super::super::guest_mem::set_thread_tlm_backing(tlm_page_of(base as u64).unwrap());
        assert_eq!(loader.paging_violation(&vcpu), None, "as set up");
        vcpu.sregs.cr3 = 0xF000_0000;
        assert_eq!(loader.paging_violation(&vcpu), Some(PagingStatus::ForeignCr3(0xF000_0000)));
        vcpu.sregs.cr3 = base as u64;
        vcpu.sregs.cr0 &= !CR0_PG;
        assert_eq!(loader.paging_violation(&vcpu), Some(PagingStatus::Off));
        vcpu.sregs.cr0 |= CR0_PG;
        loader.guest_write::<u32>(base + TABLE_OFFSET, 0).unwrap();
        assert_eq!(loader.paging_violation(&vcpu), Some(PagingStatus::Damaged));
        super::super::guest_mem::set_thread_tlm_backing(0);
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
    }
}
