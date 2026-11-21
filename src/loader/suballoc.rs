// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Memory suballocation: `DosSubSetMem`, `DosSubAllocMem`, `DosSubFreeMem`,
//! `DosSubUnsetMem` (DOSCALLS 344–347).
//!
//! Behaviour from IBM's *Control Program Programming Reference* and
//! `<bsememf.h>` / `<bseerr.h>`:
//! - `DosSubSetMem(pOffset, flags, size)` makes `size` bytes at `pOffset`
//!   a pool: `DOSSUB_INIT` initialises it, `DOSSUB_GROW` makes an existing
//!   pool larger (`ERROR_DOSSUB_SHRINK` for a smaller size; `DOSSUB_INIT`
//!   has no meaning then), `DOSSUB_SPARSE_OBJ` says the pages start
//!   uncommitted (it must match the pool's when growing),
//!   `DOSSUB_SERIALIZE` serialises access; without `DOSSUB_INIT` the call
//!   attaches to a pool set up before. Sizes are rounded down to a multiple
//!   of 8; 64 bytes are control information, so a pool has at least 72;
//! - `DosSubAllocMem(pOffset, &pBlock, size)` returns a block of `size`
//!   bytes rounded up to a multiple of 8, at most the pool size less 64;
//!   `ERROR_DOSSUB_NOMEM` when no free space is large enough;
//! - `DosSubFreeMem(pOffset, pBlock, size)` returns a block (its size
//!   rounded up the same way); `ERROR_DOSSUB_OVERLAP` when it overlaps free
//!   space;
//! - `DosSubUnsetMem(pOffset)` ends the pool (before its memory is freed);
//!   a pool whose memory `DosFreeMem` frees without it ends then
//!   (`forget_range`), so the memory can hold a new pool later.
//!
//! The pool's bookkeeping lives on the host (`SubAllocTable`), never in
//! guest memory: the 64 control bytes stay reserved but unused, and a guest
//! cannot corrupt the allocator. Pool counts and free-list sizes are
//! bounded. Warpine commits all memory, so `DOSSUB_SPARSE_OBJ` is only
//! recorded (and checked when growing); every call is serialised, so
//! `DOSSUB_SERIALIZE` changes nothing.
//!
//! Warpine's choices where the documents say nothing:
//! - unknown flag bits, and a `DOSSUB_GROW` whose `DOSSUB_SPARSE_OBJ`
//!   differs from the pool's, are `ERROR_DOSSUB_BADFLAG`;
//! - a pool smaller than 72 bytes, outside guest memory, at address 0, or
//!   — without `DOSSUB_INIT` — not set up before is
//!   `ERROR_INVALID_PARAMETER`; growing an unknown pool likewise;
//! - a new or grown pool that overlaps another pool is `ERROR_DOSSUB_OVERLAP`;
//! - a block size of 0 is `ERROR_INVALID_PARAMETER`, one above the pool size
//!   less 64 `ERROR_DOSSUB_BADSIZE`;
//! - a pool that was never set up (or was unset) is `ERROR_DOSSUB_CORRUPTED`
//!   for alloc, free and unset (OS/2 would find no control information);
//! - freeing a block outside the pool's usable space or not on an 8-byte
//!   boundary of it is `ERROR_INVALID_PARAMETER`;
//! - free space is handed out first fit, lowest address first;
//! - each attach (`DosSubSetMem` without `DOSSUB_INIT`) needs its own
//!   `DosSubUnsetMem`; the last one ends the pool.

use std::collections::BTreeMap;

use super::constants::*;
use super::mutex_ext::MutexExt;
use log::debug;

/// Bytes of control information at the start of every pool.
pub const DOSSUB_CONTROL_SIZE: u32 = 64;
/// Allocation granularity.
pub const DOSSUB_GRANULE: u32 = 8;
/// Smallest pool: the control information plus one granule.
pub const DOSSUB_MIN_POOL: u32 = DOSSUB_CONTROL_SIZE + DOSSUB_GRANULE;
/// Most pools a process has at once.
pub const MAX_SUBALLOC_POOLS: usize = 1024;
/// Most separate free ranges in one pool (a guest cannot make the host
/// bookkeeping grow without bound by fragmenting a large pool).
pub const MAX_FREE_RANGES: usize = 65536;
const VALID_FLAGS: u32 = DOSSUB_INIT | DOSSUB_GROW | DOSSUB_SPARSE_OBJ | DOSSUB_SERIALIZE;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pool {
    size: u32,
    sparse: bool,
    /// `DosSubSetMem` calls not yet matched by `DosSubUnsetMem`.
    attaches: u32,
    /// Free ranges: start address → length (merged, non-adjacent).
    free: BTreeMap<u32, u32>,
}

impl Pool {
    fn largest_block(&self) -> u32 {
        self.size - DOSSUB_CONTROL_SIZE
    }
}

/// `size` rounded up to the granule; `None` on overflow.
fn round_up(size: u32) -> Option<u32> {
    size.checked_next_multiple_of(DOSSUB_GRANULE)
}

/// The process's suballocation pools, by base address (pure bookkeeping).
#[derive(Debug, Default)]
pub struct SubAllocTable {
    pools: BTreeMap<u32, Pool>,
}

impl SubAllocTable {
    /// Whether `[start, end)` overlaps a pool other than the one at `except`.
    fn overlaps_pool(&self, start: u32, end: u32, except: Option<u32>) -> bool {
        self.pools.iter().any(|(&b, p)| Some(b) != except && b < end && start < b + p.size)
    }

    /// `DosSubSetMem`; `mem_size` bounds the pool (guest memory size).
    pub fn set(&mut self, base: u32, flags: u32, size: u32, mem_size: u32) -> u32 {
        if flags & !VALID_FLAGS != 0 {
            return ERROR_DOSSUB_BADFLAG;
        }
        let size = size & !(DOSSUB_GRANULE - 1);
        let sparse = flags & DOSSUB_SPARSE_OBJ != 0;
        if flags & DOSSUB_GROW != 0 {
            return self.grow(base, size, sparse, mem_size);
        }
        if flags & DOSSUB_INIT == 0 {
            // Attach to a pool set up before.
            return match self.pools.get_mut(&base) {
                Some(p) => { p.attaches = p.attaches.saturating_add(1); NO_ERROR }
                None => ERROR_INVALID_PARAMETER,
            };
        }
        if base == 0 || size < DOSSUB_MIN_POOL {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(end) = base.checked_add(size).filter(|&e| e <= mem_size) else {
            return ERROR_INVALID_PARAMETER;
        };
        if self.overlaps_pool(base, end, None) {
            return ERROR_DOSSUB_OVERLAP;
        }
        if self.pools.len() >= MAX_SUBALLOC_POOLS {
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        let usable = base + DOSSUB_CONTROL_SIZE;
        self.pools.insert(base, Pool {
            size, sparse, attaches: 1,
            free: BTreeMap::from([(usable, end - usable)]),
        });
        NO_ERROR
    }

    fn grow(&mut self, base: u32, size: u32, sparse: bool, mem_size: u32) -> u32 {
        let Some(pool) = self.pools.get(&base) else { return ERROR_INVALID_PARAMETER };
        if pool.sparse != sparse {
            return ERROR_DOSSUB_BADFLAG;
        }
        if size < pool.size {
            return ERROR_DOSSUB_SHRINK;
        }
        let old_end = base + pool.size;
        let Some(end) = base.checked_add(size).filter(|&e| e <= mem_size) else {
            return ERROR_INVALID_PARAMETER;
        };
        if self.overlaps_pool(base, end, Some(base)) {
            return ERROR_DOSSUB_OVERLAP;
        }
        let pool = self.pools.get_mut(&base).expect("pool looked up above");
        pool.size = size;
        if end > old_end {
            insert_free(&mut pool.free, old_end, end - old_end);
        }
        NO_ERROR
    }

    /// `DosSubAllocMem`: the address of a new block.
    pub fn alloc(&mut self, base: u32, size: u32) -> Result<u32, u32> {
        let pool = self.pools.get_mut(&base).ok_or(ERROR_DOSSUB_CORRUPTED)?;
        if size == 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let size = round_up(size).filter(|&s| s <= pool.largest_block()).ok_or(ERROR_DOSSUB_BADSIZE)?;
        let (&start, &len) = pool.free.iter().find(|&(_, &len)| len >= size).ok_or(ERROR_DOSSUB_NOMEM)?;
        pool.free.remove(&start);
        if len > size {
            pool.free.insert(start + size, len - size);
        }
        Ok(start)
    }

    /// `DosSubFreeMem`.
    pub fn free(&mut self, base: u32, block: u32, size: u32) -> u32 {
        let Some(pool) = self.pools.get_mut(&base) else { return ERROR_DOSSUB_CORRUPTED };
        if size == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(size) = round_up(size).filter(|&s| s <= pool.largest_block()) else {
            return ERROR_DOSSUB_BADSIZE;
        };
        let usable = base + DOSSUB_CONTROL_SIZE;
        let pool_end = base + pool.size;
        let inside = block >= usable && (block - usable).is_multiple_of(DOSSUB_GRANULE)
            && block.checked_add(size).is_some_and(|e| e <= pool_end);
        if !inside {
            return ERROR_INVALID_PARAMETER;
        }
        let end = block + size;
        // The free range starting at or before the block, and the next one.
        let before = pool.free.range(..=block).next_back().map(|(&s, &l)| (s, l));
        let after = pool.free.range(block..).next().map(|(&s, &l)| (s, l));
        if before.is_some_and(|(s, l)| s + l > block) || after.is_some_and(|(s, _)| s < end) {
            return ERROR_DOSSUB_OVERLAP;
        }
        let joins = before.is_some_and(|(s, l)| s + l == block) || after.is_some_and(|(s, _)| s == end);
        if !joins && pool.free.len() >= MAX_FREE_RANGES {
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        insert_free(&mut pool.free, block, size);
        NO_ERROR
    }

    /// `DosSubUnsetMem`.
    pub fn unset(&mut self, base: u32) -> u32 {
        let Some(pool) = self.pools.get_mut(&base) else { return ERROR_DOSSUB_CORRUPTED };
        pool.attaches = pool.attaches.saturating_sub(1);
        if pool.attaches == 0 {
            self.pools.remove(&base);
        }
        NO_ERROR
    }

    /// `DosFreeMem` freed `[start, start + len)`: pools that overlap it end,
    /// whatever their attach count, so a new pool may be set up there (the
    /// reference asks for `DosSubUnsetMem` first; a program that skips it
    /// would otherwise leave a stale pool behind). How many ended.
    pub fn forget_range(&mut self, start: u32, len: u32) -> usize {
        let end = start.saturating_add(len);
        let before = self.pools.len();
        self.pools.retain(|&b, p| !(b < end && start < b.saturating_add(p.size)));
        before - self.pools.len()
    }

    /// Bytes free in the pool at `base` (tests and tracing).
    pub fn free_bytes(&self, base: u32) -> Option<u32> {
        self.pools.get(&base).map(|p| p.free.values().sum())
    }
}

/// Add free range `[start, start + len)`, which overlaps no free range,
/// merging it with adjacent ones.
fn insert_free(free: &mut BTreeMap<u32, u32>, mut start: u32, mut len: u32) {
    if let Some((&s, &l)) = free.range(..start).next_back() && s + l == start {
        free.remove(&s);
        start = s;
        len += l;
    }
    if let Some(&l) = free.get(&(start + len)) {
        free.remove(&(start + len));
        len += l;
    }
    free.insert(start, len);
}

impl super::Loader {
    fn guest_mem_limit(&self) -> u32 {
        u32::try_from(self.shared.guest_mem.size()).unwrap_or(u32::MAX)
    }

    /// DosSubSetMem (ordinal 344).
    pub fn dos_sub_set_mem(&self, base: u32, flags: u32, size: u32) -> u32 {
        let rc = self.shared.suballoc.lock_or_recover().set(base, flags, size, self.guest_mem_limit());
        debug!("  DosSubSetMem({:#x}, flags={:#x}, size={}) -> {}", base, flags, size, rc);
        rc
    }

    /// DosSubAllocMem (ordinal 345).
    pub fn dos_sub_alloc_mem(&self, base: u32, pp_block: u32, size: u32) -> u32 {
        // The result must be writable before a block is taken.
        if pp_block == 0 || self.guest_read::<u32>(pp_block).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        match self.shared.suballoc.lock_or_recover().alloc(base, size) {
            Ok(block) => {
                self.guest_write::<u32>(pp_block, block);
                NO_ERROR
            }
            Err(e) => e,
        }
    }

    /// DosSubFreeMem (ordinal 346).
    pub fn dos_sub_free_mem(&self, base: u32, block: u32, size: u32) -> u32 {
        self.shared.suballoc.lock_or_recover().free(base, block, size)
    }

    /// DosSubUnsetMem (ordinal 347).
    pub fn dos_sub_unset_mem(&self, base: u32) -> u32 {
        self.shared.suballoc.lock_or_recover().unset(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x0200_0000;
    const MEM: u32 = 0x1000_0000;

    fn pool(size: u32) -> SubAllocTable {
        let mut t = SubAllocTable::default();
        assert_eq!(t.set(BASE, DOSSUB_INIT, size, MEM), NO_ERROR);
        t
    }

    #[test]
    fn set_checks_flags_size_and_range() {
        let mut t = SubAllocTable::default();
        assert_eq!(t.set(BASE, DOSSUB_INIT | 0x10, 4096, MEM), ERROR_DOSSUB_BADFLAG);
        assert_eq!(t.set(BASE, DOSSUB_INIT, DOSSUB_MIN_POOL - 1, MEM), ERROR_INVALID_PARAMETER);
        assert_eq!(t.set(0, DOSSUB_INIT, 4096, MEM), ERROR_INVALID_PARAMETER);
        assert_eq!(t.set(MEM - 64, DOSSUB_INIT, 4096, MEM), ERROR_INVALID_PARAMETER);
        assert_eq!(t.set(u32::MAX - 8, DOSSUB_INIT, 4096, u32::MAX), ERROR_INVALID_PARAMETER);
        assert_eq!(t.set(BASE, 0, 4096, MEM), ERROR_INVALID_PARAMETER, "attach to nothing");
        assert_eq!(t.set(BASE, DOSSUB_GROW, 8192, MEM), ERROR_INVALID_PARAMETER, "grow nothing");
        // 79 rounds down to 72, the smallest pool: one 8-byte block.
        assert_eq!(t.set(BASE, DOSSUB_INIT | DOSSUB_SERIALIZE, 79, MEM), NO_ERROR);
        assert_eq!(t.free_bytes(BASE), Some(8));
        assert_eq!(t.set(BASE + 64, DOSSUB_INIT, 4096, MEM), ERROR_DOSSUB_OVERLAP);
    }

    #[test]
    fn alloc_rounds_to_eight_and_fills_the_pool() {
        let mut t = pool(1024 + DOSSUB_CONTROL_SIZE);
        let a = t.alloc(BASE, 57).unwrap();
        assert_eq!(a, BASE + DOSSUB_CONTROL_SIZE);
        let b = t.alloc(BASE, 1).unwrap();
        assert_eq!(b, a + 64, "57 bytes took 64");
        assert_eq!(t.free_bytes(BASE), Some(1024 - 72));
        assert_eq!(t.alloc(BASE, 0), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(t.alloc(BASE, 1025), Err(ERROR_DOSSUB_BADSIZE));
        assert_eq!(t.alloc(BASE, u32::MAX), Err(ERROR_DOSSUB_BADSIZE));
        assert_eq!(t.alloc(BASE, 1024), Err(ERROR_DOSSUB_NOMEM));
        assert_eq!(t.alloc(BASE, 1024 - 72).unwrap(), b + 8);
        assert_eq!(t.alloc(BASE, 8), Err(ERROR_DOSSUB_NOMEM));
        assert_eq!(t.alloc(BASE + 8, 8), Err(ERROR_DOSSUB_CORRUPTED));
    }

    #[test]
    fn free_merges_and_detects_overlap() {
        let mut t = pool(256 + DOSSUB_CONTROL_SIZE);
        let a = t.alloc(BASE, 64).unwrap();
        let b = t.alloc(BASE, 64).unwrap();
        let c = t.alloc(BASE, 64).unwrap();
        assert_eq!(t.free(BASE, b, 64), NO_ERROR);
        assert_eq!(t.free(BASE, b, 64), ERROR_DOSSUB_OVERLAP, "freed twice");
        assert_eq!(t.free(BASE, b + 8, 8), ERROR_DOSSUB_OVERLAP, "inside free space");
        assert_eq!(t.free(BASE, a, 72), ERROR_DOSSUB_OVERLAP, "runs into free space");
        assert_eq!(t.free(BASE, a + 4, 8), ERROR_INVALID_PARAMETER, "not on 8 bytes");
        assert_eq!(t.free(BASE, BASE, 8), ERROR_INVALID_PARAMETER, "control information");
        assert_eq!(t.free(BASE, c, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(t.free(BASE, c, 257), ERROR_DOSSUB_BADSIZE);
        assert_eq!(t.free(BASE, c + 64, 64), ERROR_DOSSUB_OVERLAP, "the free tail");
        assert_eq!(t.free(BASE, c + 64, 128), ERROR_INVALID_PARAMETER, "past the pool's end");
        assert_eq!(t.free(BASE, a, 64), NO_ERROR);
        assert_eq!(t.free(BASE, c, 64), NO_ERROR);
        // All one range again: the whole usable space can be taken.
        assert_eq!(t.free_bytes(BASE), Some(256));
        assert_eq!(t.alloc(BASE, 256), Ok(a));
        assert_eq!(t.free(BASE + 1, a, 8), ERROR_DOSSUB_CORRUPTED);
    }

    #[test]
    fn grow_adds_space_and_checks_flags() {
        let mut t = SubAllocTable::default();
        assert_eq!(t.set(BASE, DOSSUB_INIT | DOSSUB_SPARSE_OBJ, 128 + 64, MEM), NO_ERROR);
        let a = t.alloc(BASE, 128).unwrap();
        assert_eq!(t.alloc(BASE, 8), Err(ERROR_DOSSUB_NOMEM));
        assert_eq!(t.set(BASE, DOSSUB_GROW, 4096, MEM), ERROR_DOSSUB_BADFLAG, "sparse differs");
        assert_eq!(t.set(BASE, DOSSUB_GROW | DOSSUB_SPARSE_OBJ, 64, MEM), ERROR_DOSSUB_SHRINK);
        assert_eq!(t.set(BASE, DOSSUB_GROW | DOSSUB_SPARSE_OBJ, MEM, MEM), ERROR_INVALID_PARAMETER);
        assert_eq!(t.set(BASE, DOSSUB_GROW | DOSSUB_SPARSE_OBJ | DOSSUB_INIT, 256 + 64, MEM), NO_ERROR);
        assert_eq!(t.alloc(BASE, 128), Ok(a + 128));
        assert_eq!(t.alloc(BASE, 8), Err(ERROR_DOSSUB_NOMEM));
        assert_eq!(t.alloc(BASE, 256 + 8), Err(ERROR_DOSSUB_BADSIZE));
        assert_eq!(t.free(BASE, a, 256), NO_ERROR, "freed across the old end");
        assert_eq!(t.free_bytes(BASE), Some(256));
        // Growing into another pool overlaps it.
        assert_eq!(t.set(BASE + 4096, DOSSUB_INIT, 4096, MEM), NO_ERROR);
        assert_eq!(t.set(BASE, DOSSUB_GROW | DOSSUB_SPARSE_OBJ, 8192, MEM), ERROR_DOSSUB_OVERLAP);
    }

    #[test]
    fn attach_and_unset_count() {
        let mut t = pool(4096);
        assert_eq!(t.set(BASE, 0, 4096, MEM), NO_ERROR, "attach");
        assert_eq!(t.unset(BASE), NO_ERROR);
        assert!(t.alloc(BASE, 8).is_ok(), "still set");
        assert_eq!(t.unset(BASE), NO_ERROR);
        assert_eq!(t.unset(BASE), ERROR_DOSSUB_CORRUPTED);
        assert_eq!(t.alloc(BASE, 8), Err(ERROR_DOSSUB_CORRUPTED));
        assert_eq!(t.set(BASE, DOSSUB_INIT, 4096, MEM), NO_ERROR, "set up again");
    }

    #[test]
    fn pools_and_free_ranges_are_bounded() {
        let mut t = SubAllocTable::default();
        for i in 0..MAX_SUBALLOC_POOLS as u32 {
            assert_eq!(t.set(BASE + i * 128, DOSSUB_INIT, 128, MEM), NO_ERROR);
        }
        assert_eq!(t.set(BASE + 0x100_0000, DOSSUB_INIT, 128, MEM), ERROR_NOT_ENOUGH_MEMORY);

        // Fragment a pool: free every other 8-byte block.
        let n = MAX_FREE_RANGES as u32 + 2;
        let size = 2 * 8 * n + DOSSUB_CONTROL_SIZE;
        let mut t = SubAllocTable::default();
        assert_eq!(t.set(BASE, DOSSUB_INIT, size, MEM), NO_ERROR);
        let first = t.alloc(BASE, size - DOSSUB_CONTROL_SIZE).unwrap();
        let mut last = NO_ERROR;
        for i in 0..n {
            last = t.free(BASE, first + i * 16, 8);
            if last != NO_ERROR { break; }
        }
        assert_eq!(last, ERROR_NOT_ENOUGH_MEMORY);
        // A free that joins a range still works at the limit.
        assert_eq!(t.free(BASE, first + 8, 8), NO_ERROR);
    }

    #[test]
    fn loader_calls_check_the_result_pointer() {
        let loader = super::super::Loader::new_mock();
        let base = loader.shared.mem_mgr.lock_or_recover().alloc(8192).unwrap();
        assert_eq!(loader.dos_sub_set_mem(base, DOSSUB_INIT, 8192), NO_ERROR);
        assert_eq!(loader.dos_sub_alloc_mem(base, 0, 16), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_sub_alloc_mem(base, u32::MAX - 1, 16), ERROR_INVALID_PARAMETER);
        let out = base + 8192 - 4; // inside the pool's last bytes: fine for the test
        assert_eq!(loader.dos_sub_alloc_mem(base, out, 16), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(out), Some(base + DOSSUB_CONTROL_SIZE));
        assert_eq!(loader.dos_sub_free_mem(base, base + DOSSUB_CONTROL_SIZE, 16), NO_ERROR);
        assert_eq!(loader.dos_sub_unset_mem(base), NO_ERROR);
        assert_eq!(loader.dos_sub_set_mem(base, DOSSUB_INIT, 0x1000_0000), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn forget_range_ends_overlapping_pools_only() {
        let mut t = pool(4096);
        assert_eq!(t.set(BASE, 0, 4096, MEM), NO_ERROR, "attached twice");
        assert_eq!(t.set(BASE + 8192, DOSSUB_INIT, 4096, MEM), NO_ERROR);
        assert_eq!(t.forget_range(BASE + 4096, 4096), 0, "between the pools");
        assert_eq!(t.forget_range(BASE, 4096), 1);
        assert_eq!(t.free_bytes(BASE), None, "gone whatever its attach count");
        assert!(t.free_bytes(BASE + 8192).is_some());
        assert_eq!(t.forget_range(BASE + 8192 + 4000, 8), 1, "any overlap ends it");
        assert_eq!(t.forget_range(u32::MAX - 4, 100), 0, "no overflow");
    }

    #[test]
    fn free_mem_without_unset_ends_the_pool() {
        let loader = super::super::Loader::new_mock();
        const PPB: u32 = 0x0000_1000;
        assert_eq!(loader.dos_alloc_mem(PPB, 8192, PAG_COMMIT | PAG_READ | PAG_WRITE), NO_ERROR);
        let base = loader.guest_read::<u32>(PPB).unwrap();
        assert_eq!(loader.dos_sub_set_mem(base, DOSSUB_INIT, 8192), NO_ERROR);
        assert_eq!(loader.dos_sub_set_mem(base, DOSSUB_INIT, 8192), ERROR_DOSSUB_OVERLAP);
        // Freed without DosSubUnsetMem.
        assert_eq!(loader.dos_free_mem(base), NO_ERROR);
        assert_eq!(loader.dos_sub_alloc_mem(base, base, 16), ERROR_DOSSUB_CORRUPTED);
        // The same memory again: a new pool can be set up there.
        assert_eq!(loader.dos_alloc_mem(PPB, 8192, PAG_COMMIT | PAG_READ | PAG_WRITE), NO_ERROR);
        let again = loader.guest_read::<u32>(PPB).unwrap();
        assert_eq!(again, base, "the freed block is handed out again");
        assert_eq!(loader.dos_sub_set_mem(again, DOSSUB_INIT, 8192), NO_ERROR);
        assert_eq!(loader.dos_sub_unset_mem(again), NO_ERROR);
    }
}
