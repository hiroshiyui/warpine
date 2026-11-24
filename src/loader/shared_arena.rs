// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Shared memory between the Warpine processes of a session (ADR-0049).
//!
//! Every OS/2 process is a host process of its own (ADR-0020), so OS/2's
//! shared memory needs host memory several guests see. The top
//! `SHARED_ARENA_SIZE` bytes of every process's guest memory, from
//! `SHARED_ARENA_BASE`, are one sealed `memfd` mapped shared over those
//! guest pages (`GuestMemory::map_shared`): the *arena*. The first process
//! of a session makes it; children inherit the descriptor
//! (`WARPINE_ARENA_FD`, close-on-exec cleared only in the child being
//! started, as for pipes) and map the same file at the same place, so a
//! shared block has one address in every process, as in OS/2.
//!
//! Which blocks exist, and which processes hold them, is the table
//! `shmem` in the session's private directory (`session_threads.rs`),
//! read and written under the directory's lock. A holder is an OS/2
//! process id; it counts while the session has a live record for it (a
//! running process, or a child being started), so a killed process's
//! blocks are released when the table is next used. A block without
//! holders is gone: its pages are punched out of the file (zero, memory
//! given back) and its range and name can be had again.
//!
//! Everything read from the table is treated as hostile: lines are parsed
//! strictly and bounded, blocks must lie in the arena without overlapping,
//! and a malformed line is dropped. The guest can write anywhere in the
//! arena (there is no paging), but the table is not in guest memory.
//!
//! The memfd is sealed against shrinking and growing, so no host access
//! through the mapping can fault past its end (a `SIGBUS` would end the
//! process). A descriptor named in `WARPINE_ARENA_FD` is used only when it
//! is such a sealed memfd of exactly the arena's size; the table's header
//! names the arena (device and inode), so a process that somehow has
//! another arena does not share blocks under the same addresses.
//!
//! Limits (Warpine's): the guest can read and write every block of the
//! arena whether or not its process holds it, and the access flags of a
//! get or give are checked but not enforced.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::guest_mem::{ArenaGuard, HeldBlock};
use super::paging::{process_pte, process_pte_at, PageAccess};
use super::session_threads::SHARED_MEM_TABLE;

/// The descriptor of the session's arena a child inherits.
pub const ARENA_FD_ENV: &str = "WARPINE_ARENA_FD";
/// Page size of the arena's blocks.
const PAGE: u32 = 4096;
/// Most blocks the arena holds.
pub const MAX_SHARED_BLOCKS: usize = 1024;
/// Most processes holding one block.
pub const MAX_HOLDERS: usize = 64;
/// Longest block name, in bytes (UTF-8 of the upper-case name).
pub const MAX_NAME_BYTES: usize = 255;
/// Largest table file: a header and `MAX_SHARED_BLOCKS` full lines.
const MAX_TABLE_BYTES: usize = 64 + MAX_SHARED_BLOCKS * (3 * 9 + 2 * MAX_NAME_BYTES + 2 + MAX_HOLDERS * 11 + 1);
/// First line of a table.
const HEADER: &str = "warpine-shmem 2";
/// The name prefix of named shared memory.
pub const SHAREMEM_PREFIX: &str = "\\SHAREMEM\\";
/// The allocation flags a block keeps.
const BLOCK_FLAGS: u32 = PAG_READ | PAG_WRITE | PAG_EXECUTE | PAG_GUARD | PAG_COMMIT
    | OBJ_TILE | OBJ_GETTABLE | OBJ_GIVEABLE | OBJ_ANY;

/// A shared block: where it is in the arena, its flags, its name and the
/// processes holding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaBlock {
    /// Bytes from `SHARED_ARENA_BASE`, page aligned.
    pub offset: u32,
    /// Bytes, a whole number of pages.
    pub size: u32,
    /// `DosAllocSharedMem` flags.
    pub flags: u32,
    /// Upper-case `\SHAREMEM\…` name.
    pub name: Option<String>,
    /// OS/2 process ids, sorted, no duplicates, never empty.
    pub holders: Vec<u32>,
    /// The holders that may only read it (they got or were given it
    /// without `PAG_WRITE`): sorted, each one a holder.
    pub read_only: Vec<u32>,
}

/// Which arena a table belongs to: its memfd's device and inode.
pub type ArenaId = (u64, u64);

/// The session's shared blocks, by offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArenaTable {
    pub arena: Option<ArenaId>,
    blocks: Vec<ArenaBlock>,
}

fn hex_name(name: &str) -> String {
    name.bytes().fold(String::with_capacity(name.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex_name(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) || text.len() > 2 * MAX_NAME_BYTES || text.is_empty() { return None; }
    let bytes: Option<Vec<u8>> = (0..text.len()).step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect();
    let name = String::from_utf8(bytes?).ok()?;
    valid_name(&name).then_some(name)
}

/// Whether `name` is an upper-case `\SHAREMEM\…` name within the bounds.
fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME_BYTES && name.len() > SHAREMEM_PREFIX.len()
        && name.starts_with(SHAREMEM_PREFIX) && name.to_ascii_uppercase() == name
        && !name.chars().any(char::is_control)
}

/// The canonical form of a guest's shared-memory name (`/` read as `\`,
/// upper case), or `None` when it is not a valid `\SHAREMEM\…` name.
pub fn canonical_name(name: &str) -> Option<String> {
    let n = name.replace('/', "\\").to_ascii_uppercase();
    valid_name(&n).then_some(n)
}

fn hex_u32(s: &str) -> Option<u32> {
    (!s.is_empty() && s.len() <= 8 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

fn dec_u64(s: &str) -> Option<u64> {
    (!s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok()).flatten()
}

impl ArenaBlock {
    fn parse(line: &str) -> Option<ArenaBlock> {
        let mut parts = line.split(' ');
        let offset = hex_u32(parts.next()?)?;
        let size = hex_u32(parts.next()?)?;
        let flags = hex_u32(parts.next()?)?;
        let name = match parts.next()? {
            "-" => None,
            h => Some(unhex_name(h)?),
        };
        let holders_text = parts.next()?;
        if parts.next().is_some() { return None; }
        let mut holders = Vec::new();
        let mut read_only = Vec::new();
        for h in holders_text.split(',') {
            // `12` holds the block with its access; `12r` may only read it.
            let (h, reads) = h.strip_suffix('r').map_or((h, false), |h| (h, true));
            let pid = u32::try_from(dec_u64(h)?).ok()?;
            if pid == 0 || pid > MAX_PROCESS_ID || holders.len() >= MAX_HOLDERS { return None; }
            holders.push(pid);
            if reads { read_only.push(pid); }
        }
        holders.sort_unstable();
        holders.dedup();
        read_only.sort_unstable();
        read_only.dedup();
        let end = offset.checked_add(size)?;
        if !offset.is_multiple_of(PAGE) || size == 0 || !size.is_multiple_of(PAGE)
            || end > SHARED_ARENA_SIZE || flags & !BLOCK_FLAGS != 0 {
            return None;
        }
        Some(ArenaBlock { offset, size, flags, name, holders, read_only })
    }

    fn encode(&self, out: &mut String) {
        let name = self.name.as_deref().map_or_else(|| "-".to_string(), hex_name);
        let holders: Vec<String> = self.holders.iter()
            .map(|p| if self.read_only.contains(p) { format!("{p}r") } else { p.to_string() }).collect();
        let _ = writeln!(out, "{:x} {:x} {:x} {} {}", self.offset, self.size, self.flags, name, holders.join(","));
    }

    /// The guest address of the block.
    pub fn addr(&self) -> u32 { SHARED_ARENA_BASE + self.offset }

    fn end(&self) -> u32 { self.offset + self.size }

    /// Whether process `pid` holds the block.
    pub fn held_by(&self, pid: u32) -> bool { self.holders.binary_search(&pid).is_ok() }

    /// What process `pid`'s guest code may do with the block's pages
    /// (ADR-0049 addendum): nothing unless it holds it; write when the
    /// block allows it and `pid` did not get it read-only; read otherwise
    /// (`PAG_EXECUTE` alone reads too: 32-bit paging cannot tell them apart).
    pub fn access_of(&self, pid: u32) -> PageAccess {
        if !self.held_by(pid) {
            PageAccess::None
        } else if self.flags & PAG_WRITE != 0 && self.read_only.binary_search(&pid).is_err() {
            PageAccess::ReadWrite
        } else {
            PageAccess::Read
        }
    }
}

/// What a get or give asks: the holder to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldError {
    /// No block starts at that address.
    NoBlock,
    /// The block has `MAX_HOLDERS` already.
    Full,
}

impl ArenaTable {
    /// The table in `bytes`: malformed lines, blocks outside the arena,
    /// overlapping ones and duplicate names are dropped.
    pub fn parse(bytes: &[u8]) -> ArenaTable {
        let mut table = ArenaTable::default();
        let Ok(text) = std::str::from_utf8(bytes) else { return table };
        let mut lines = text.lines();
        let Some(header) = lines.next() else { return table };
        let mut head = header.split(' ');
        if head.next() != Some("warpine-shmem") || head.next() != Some("2") { return table; }
        table.arena = match (head.next().and_then(dec_u64), head.next().and_then(dec_u64), head.next()) {
            (Some(dev), Some(ino), None) => Some((dev, ino)),
            _ => None,
        };
        let mut blocks: Vec<ArenaBlock> = lines.take(MAX_SHARED_BLOCKS).filter_map(ArenaBlock::parse).collect();
        blocks.sort_by_key(|b| b.offset);
        let mut names = HashSet::new();
        for b in blocks {
            let overlaps = table.blocks.last().is_some_and(|last| last.end() > b.offset);
            let dup = b.name.as_ref().is_some_and(|n| !names.insert(n.clone()));
            if !overlaps && !dup {
                table.blocks.push(b);
            }
        }
        table
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::from(HEADER);
        if let Some((dev, ino)) = self.arena {
            let _ = write!(out, " {dev} {ino}");
        }
        out.push('\n');
        for b in &self.blocks {
            b.encode(&mut out);
        }
        out.into_bytes()
    }

    pub fn blocks(&self) -> &[ArenaBlock] { &self.blocks }

    /// Drop holders that are not `live`; the ranges (offset, size) of
    /// blocks left without holders, which are gone.
    pub fn sweep(&mut self, live: &HashSet<u32>) -> Vec<(u32, u32)> {
        let mut freed = Vec::new();
        self.blocks.retain_mut(|b| {
            b.holders.retain(|p| live.contains(p));
            b.read_only.retain(|p| live.contains(p));
            if b.holders.is_empty() { freed.push((b.offset, b.size)); }
            !b.holders.is_empty()
        });
        freed
    }

    /// A new block of `size` bytes (rounded up to pages) held by `holder`,
    /// at the lowest free range that fits; its offset (`None`: no room).
    pub fn alloc(&mut self, size: u32, flags: u32, name: Option<String>, holder: u32) -> Option<u32> {
        if self.blocks.len() >= MAX_SHARED_BLOCKS || size == 0 { return None; }
        let size = size.checked_add(PAGE - 1)? & !(PAGE - 1);
        let mut at = 0u32;
        let mut index = self.blocks.len();
        for (i, b) in self.blocks.iter().enumerate() {
            if b.offset - at >= size { index = i; break; }
            at = b.end();
        }
        if index == self.blocks.len() && SHARED_ARENA_SIZE - at < size { return None; }
        self.blocks.insert(index, ArenaBlock {
            offset: at, size, flags: flags & BLOCK_FLAGS, name, holders: vec![holder], read_only: Vec::new(),
        });
        Some(at)
    }

    pub fn by_name(&self, name: &str) -> Option<&ArenaBlock> {
        self.blocks.iter().find(|b| b.name.as_deref() == Some(name))
    }

    pub fn at(&self, offset: u32) -> Option<&ArenaBlock> {
        self.blocks.iter().find(|b| b.offset == offset)
    }

    /// The block holding byte `offset` of the arena.
    pub fn containing(&self, offset: u32) -> Option<&ArenaBlock> {
        self.blocks.iter().find(|b| (b.offset..b.end()).contains(&offset))
    }

    /// Add `pid` to the holders of the block at `offset`, with the block's
    /// access (already one: no change but the access, see [`Self::hold_as`]).
    pub fn hold(&mut self, offset: u32, pid: u32) -> Result<(), HoldError> {
        self.hold_as(offset, pid, true)
    }

    /// Add `pid` to the holders of the block at `offset`; it may write the
    /// block (if the block allows it) when `write`. A holder asking again
    /// keeps the larger access (unverified).
    pub fn hold_as(&mut self, offset: u32, pid: u32, write: bool) -> Result<(), HoldError> {
        let b = self.blocks.iter_mut().find(|b| b.offset == offset).ok_or(HoldError::NoBlock)?;
        let reads = b.read_only.binary_search(&pid);
        match b.holders.binary_search(&pid) {
            Ok(_) => {
                if write && let Ok(i) = reads { b.read_only.remove(i); }
            }
            Err(i) => {
                if b.holders.len() >= MAX_HOLDERS { return Err(HoldError::Full); }
                b.holders.insert(i, pid);
                if !write && let Err(j) = reads { b.read_only.insert(j, pid); }
            }
        }
        Ok(())
    }

    /// `pid` lets the block at `offset` go: `None` when it does not hold
    /// it; else the block's range when that was the last holder.
    pub fn release(&mut self, offset: u32, pid: u32) -> Option<Option<(u32, u32)>> {
        let i = self.blocks.iter().position(|b| b.offset == offset)?;
        let b = &mut self.blocks[i];
        let at = b.holders.binary_search(&pid).ok()?;
        b.holders.remove(at);
        b.read_only.retain(|&p| p != pid);
        if b.holders.is_empty() {
            let b = self.blocks.remove(i);
            return Some(Some((b.offset, b.size)));
        }
        Some(None)
    }

    /// `pid` ends: it lets all its blocks go; the ranges of those gone.
    pub fn release_all(&mut self, pid: u32) -> Vec<(u32, u32)> {
        let offsets: Vec<u32> = self.blocks.iter().filter(|b| b.holders.contains(&pid)).map(|b| b.offset).collect();
        offsets.into_iter().filter_map(|o| self.release(o, pid).flatten()).collect()
    }
}

// ── The arena's host memory ──────────────────────────────────────────────

/// The seals an arena memfd carries: its size cannot change, nor its seals.
const ARENA_SEALS: libc::c_int = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;

/// The session's arena: a sealed memfd of `SHARED_ARENA_SIZE` bytes.
#[derive(Debug)]
pub struct Arena {
    fd: OwnedFd,
    id: ArenaId,
}

impl Arena {
    /// A new, zero arena.
    pub fn create() -> std::io::Result<Arena> {
        // SAFETY: the name is a NUL-terminated literal; memfd_create has no
        // other preconditions.
        let raw = unsafe {
            libc::memfd_create(c"warpine-shared-arena".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
        };
        if raw < 0 { return Err(std::io::Error::last_os_error()); }
        // SAFETY: memfd_create returned a new descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let file = std::fs::File::from(fd);
        file.set_len(u64::from(SHARED_ARENA_SIZE))?;
        let fd = OwnedFd::from(file);
        // SAFETY: fcntl on a descriptor this function owns.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, ARENA_SEALS) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Self::checked(fd)
    }

    /// The arena a parent passed in `WARPINE_ARENA_FD` (`Ok(None)`: none).
    pub fn from_env() -> std::io::Result<Option<Arena>> {
        let Some(text) = std::env::var_os(ARENA_FD_ENV) else { return Ok(None) };
        let fd = text.to_str().and_then(dec_u64).and_then(|n| RawFd::try_from(n).ok())
            .filter(|&n| n > 2)
            .ok_or_else(|| std::io::Error::other("malformed WARPINE_ARENA_FD"))?;
        // SAFETY: fcntl on any number is safe; it fails for a closed one.
        let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if fd_flags < 0 { return Err(std::io::Error::last_os_error()); }
        // SAFETY: the descriptor is open (F_GETFD succeeded) and was handed
        // to this process for the arena alone: nothing else in it uses the
        // number. It is closed on exec again from here on.
        let fd = unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            OwnedFd::from_raw_fd(fd)
        };
        Self::checked(fd).map(Some)
    }

    /// `fd` as an arena: a regular file of the arena's size carrying the
    /// arena's seals.
    fn checked(fd: OwnedFd) -> std::io::Result<Arena> {
        // SAFETY: fcntl on a descriptor owned by `fd`.
        let seals = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) };
        if seals < 0 || seals & ARENA_SEALS != ARENA_SEALS {
            return Err(std::io::Error::other("the arena is not a sealed memfd"));
        }
        let meta = std::fs::File::from(fd.try_clone()?).metadata()?;
        use std::os::unix::fs::MetadataExt;
        if !meta.is_file() || meta.len() != u64::from(SHARED_ARENA_SIZE) {
            return Err(std::io::Error::other("the arena has the wrong size"));
        }
        Ok(Arena { id: (meta.dev(), meta.ino()), fd })
    }

    pub fn id(&self) -> ArenaId { self.id }

    pub fn fd(&self) -> BorrowedFd<'_> { self.fd.as_fd() }

    /// Zero `size` bytes at `offset` and give their memory back (the
    /// pages are punched out of the file; every mapping reads zeros).
    pub fn zero(&self, offset: u32, size: u32) {
        // SAFETY: fallocate on a descriptor this arena owns; the range lies
        // inside the file (callers pass blocks of the table, which are
        // bounded by the arena), and KEEP_SIZE leaves its size alone.
        let r = unsafe {
            libc::fallocate(self.fd.as_raw_fd(), libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
                libc::off_t::from(offset), libc::off_t::from(size))
        };
        if r != 0 {
            warn!("shared arena: cannot clear {size} bytes at {offset:#x}: {}", std::io::Error::last_os_error());
        }
    }

    /// Pass the arena to the child `cmd` starts: the variable, and close-on-
    /// exec cleared on the descriptor in the forked child only.
    pub fn pass_to(&self, cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        let fd = self.fd.as_raw_fd();
        cmd.env(ARENA_FD_ENV, fd.to_string());
        // SAFETY: the hook runs in the forked child before `exec` and only
        // calls fcntl (async-signal-safe) on a descriptor the arena keeps
        // open for the life of the process.
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

// ── The session's table ──────────────────────────────────────────────────

/// What one use of the table gives: `f`'s result, the blocks this process
/// let go (offset, size) and the blocks it holds afterwards.
type TableRun<R> = (R, Vec<(u32, u32)>, Vec<HeldBlock>);

/// Why the table cannot be used.
#[derive(Debug)]
pub struct TableUnavailable;

impl super::SharedState {
    /// Run `f` on the session's shared-memory table, with the OS/2 process
    /// ids of the session's live processes, after dropping dead holders;
    /// the table is written back when `f` changed it, and the memory of
    /// blocks gone (by the sweep or by `f`, which adds their ranges to the
    /// vector it gets) is given back.
    pub fn with_arena_table<R>(&self, f: impl FnOnce(&mut ArenaTable, &HashSet<u32>, &mut Vec<(u32, u32)>) -> R)
        -> Result<R, TableUnavailable>
    {
        let arena = self.arena.get().ok_or(TableUnavailable)?;
        let slots = self.thread_slots.lock_or_recover();
        let session = slots.session().ok_or(TableUnavailable)?;
        let run = || -> std::io::Result<TableRun<R>> {
            let t = session.tables()?;
            let bytes = t.read(SHARED_MEM_TABLE, MAX_TABLE_BYTES)?.unwrap_or_default();
            let mut table = ArenaTable::parse(&bytes);
            let live = t.live_processes()?;
            let mut freed = Vec::new();
            if table.arena != Some(arena.id()) {
                // Another arena's blocks mean nothing here: take the table
                // over only when they are all gone.
                let mut theirs = table.clone();
                theirs.sweep(&live);
                if !theirs.blocks.is_empty() {
                    return Err(std::io::Error::other("the session's table belongs to another arena"));
                }
                table = ArenaTable { arena: Some(arena.id()), blocks: Vec::new() };
            } else {
                freed = table.sweep(&live);
            }
            let before = table.clone();
            let r = f(&mut table, &live, &mut freed);
            if table != before || !freed.is_empty() || bytes.is_empty() {
                t.write(SHARED_MEM_TABLE, &table.encode())?;
            }
            // Still under the lock: another process may take the range as
            // soon as it is released.
            for (offset, size) in freed {
                arena.zero(offset, size);
            }
            let me = self.process_id();
            Ok((r, lost_blocks(&before, &table, me), held_blocks(&table, me)))
        };
        match run() {
            Ok((r, lost, held)) => {
                self.guest_mem.set_arena_holds(held);
                self.unmap_arena_blocks(&lost);
                Ok(r)
            }
            Err(e) => {
                warn!("shared arena: the session's table cannot be used: {e}");
                Err(TableUnavailable)
            }
        }
    }

    /// This process's guest code no longer reaches the blocks at `ranges`
    /// (offset, size), which it let go: their pages leave the process
    /// tables and every vCPU's TLB (ADR-0049 addendum).
    fn unmap_arena_blocks(&self, ranges: &[(u32, u32)]) {
        if ranges.is_empty() || !self.process_tables_ready.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        for &(offset, size) in ranges {
            for page in (offset..offset.saturating_add(size)).step_by(PAGE as usize) {
                let addr = SHARED_ARENA_BASE + page;
                if let Some(at) = process_pte_at(addr) {
                    self.guest_mem.write_system::<u32>(at, process_pte(addr, PageAccess::None));
                }
            }
        }
        self.tlb_epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.threads.table.lock_or_recover().kick_all_but(0);
    }

    /// Map the session's arena over the top of guest memory: the one a
    /// parent passed, or a new one for the first process of a session.
    /// Without a session (or when anything fails) shared memory stays
    /// within this process.
    pub fn attach_shared_arena(&self) {
        if self.thread_slots.lock_or_recover().session().is_none() { return; }
        let arena = match Arena::from_env() {
            Ok(Some(a)) => a,
            Ok(None) => match Arena::create() {
                Ok(a) => a,
                Err(e) => { warn!("shared arena: cannot make one: {e}"); return; }
            },
            Err(e) => {
                warn!("shared arena: the one passed on is unusable ({e}); shared memory stays in this process");
                return;
            }
        };
        if let Err(e) = self.guest_mem.map_shared(SHARED_ARENA_BASE, SHARED_ARENA_SIZE as usize, arena.fd()) {
            warn!("shared arena: cannot map it: {e}");
            return;
        }
        let id = arena.id();
        let _ = self.arena.set(arena);
        // From now on API calls reach only the blocks this process holds
        // (ADR-0049 addendum); a miss reads the table again, unlocked.
        let table = self.thread_slots.lock_or_recover().session().map(|s| s.dir_path().join(SHARED_MEM_TABLE));
        if let Some(table) = table {
            let me = self.process_id();
            self.guest_mem.guard_arena(ArenaGuard {
                holds: Vec::new(),
                refresh: Box::new(move || held_in_table_file(&table, id, me)),
            });
        }
        // Make sure the table is this arena's (or say why not).
        if self.with_arena_table(|_, _, _| ()).is_err() {
            warn!("shared arena: shared memory with the other processes of the session is not available");
        }
        debug!("shared arena mapped at {SHARED_ARENA_BASE:#x}");
    }

    /// This process ends: it lets all its shared blocks go.
    pub fn release_shared_arena(&self) {
        let me = self.process_id();
        let _ = self.with_arena_table(|t, _, freed| freed.extend(t.release_all(me)));
    }
}

/// The blocks `pid` held in `before` and no longer holds in `after`, as
/// (offset, size): their pages must leave its tables.
fn lost_blocks(before: &ArenaTable, after: &ArenaTable, pid: u32) -> Vec<(u32, u32)> {
    before.blocks.iter()
        .filter(|b| b.held_by(pid))
        .filter(|b| !after.at(b.offset).is_some_and(|a| a.size == b.size && a.held_by(pid)))
        .map(|b| (b.offset, b.size))
        .collect()
}

/// The blocks `pid` holds in `table`, as guest addresses with their write
/// access (`GuestMemory::arena_allows`).
fn held_blocks(table: &ArenaTable, pid: u32) -> Vec<HeldBlock> {
    table.blocks.iter()
        .filter(|b| b.held_by(pid))
        .map(|b| (b.addr(), b.addr() + b.size, b.access_of(pid) == PageAccess::ReadWrite))
        .collect()
}

/// [`held_blocks`] of the table file at `path`, read without the
/// session's lock (the file is replaced by rename, so the read sees one
/// version), for arena `id`: none when the table is another arena's.
/// `None` when the file cannot be read.
fn held_in_table_file(path: &std::path::Path, id: ArenaId, pid: u32) -> Option<Vec<HeldBlock>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    if !f.metadata().ok()?.is_file() { return None; }
    let mut bytes = Vec::new();
    f.take(MAX_TABLE_BYTES as u64).read_to_end(&mut bytes).ok()?;
    let table = ArenaTable::parse(&bytes);
    Some(if table.arena == Some(id) { held_blocks(&table, pid) } else { Vec::new() })
}

/// Whether guest address `addr` lies in the arena.
pub fn in_arena(addr: u32) -> bool {
    (SHARED_ARENA_BASE..SHARED_ARENA_BASE.wrapping_add(SHARED_ARENA_SIZE)).contains(&addr)
}

// ── The API calls on the arena ───────────────────────────────────────────

impl super::Loader {
    /// `DosAllocSharedMem` in the arena; `None` when the session's table
    /// cannot be used (the caller then allocates within the process).
    pub(super) fn arena_alloc(&self, ppb: u32, name: Option<String>, cb: u32, flag: u32) -> Option<u32> {
        if self.guest_ptr(ppb, 4).is_none() { return Some(ERROR_INVALID_PARAMETER); }
        let me = self.shared.process_id();
        let r = self.shared.with_arena_table(|t, _, _| {
            if let Some(n) = &name && t.by_name(n).is_some() {
                return Err(ERROR_ALREADY_EXISTS);
            }
            let offset = t.alloc(cb, flag, name, me).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
            Ok((offset, t.at(offset).map_or(0, |b| b.size)))
        }).ok()?;
        Some(match r {
            Ok((offset, size)) => {
                // Zero whatever a block there before left behind.
                if let Some(arena) = self.shared.arena.get() {
                    arena.zero(offset, size);
                }
                self.guest_write::<u32>(ppb, SHARED_ARENA_BASE + offset);
                NO_ERROR
            }
            Err(e) => e,
        })
    }

    /// `DosGetNamedSharedMem` in the arena (`name` canonical); the caller
    /// may write the block only when `flag` has `PAG_WRITE`.
    pub(super) fn arena_get_named(&self, ppb: u32, name: &str, flag: u32) -> Option<u32> {
        if self.guest_ptr(ppb, 4).is_none() { return Some(ERROR_INVALID_PARAMETER); }
        let me = self.shared.process_id();
        let r = self.shared.with_arena_table(|t, _, _| {
            let offset = t.by_name(name).map(|b| b.offset).ok_or(ERROR_FILE_NOT_FOUND)?;
            t.hold_as(offset, me, flag & PAG_WRITE != 0).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
            Ok(offset)
        }).ok()?;
        Some(match r {
            Ok(offset) => { self.guest_write::<u32>(ppb, SHARED_ARENA_BASE + offset); NO_ERROR }
            Err(e) => e,
        })
    }

    /// `DosGetSharedMem` in the arena: the block at `pb` must be gettable;
    /// the caller may write it only when `flag` has `PAG_WRITE`.
    pub(super) fn arena_get(&self, pb: u32, flag: u32) -> Option<u32> {
        let me = self.shared.process_id();
        let offset = pb.wrapping_sub(SHARED_ARENA_BASE);
        let r = self.shared.with_arena_table(|t, _, _| {
            let block = t.at(offset).ok_or(ERROR_INVALID_ADDRESS)?;
            if block.flags & OBJ_GETTABLE == 0 { return Err(ERROR_ACCESS_DENIED); }
            t.hold_as(offset, me, flag & PAG_WRITE != 0).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)
        }).ok()?;
        Some(r.err().unwrap_or(NO_ERROR))
    }

    /// `DosGiveSharedMem` in the arena: the caller must hold a giveable
    /// block at `pb`; `pid` must be a live process of the session (or a
    /// child being started).
    pub(super) fn arena_give(&self, pb: u32, pid: u32, perm: u32) -> Option<u32> {
        let me = self.shared.process_id();
        let offset = pb.wrapping_sub(SHARED_ARENA_BASE);
        let r = self.shared.with_arena_table(|t, live, _| {
            let block = t.at(offset).filter(|b| b.holders.contains(&me)).ok_or(ERROR_INVALID_ADDRESS)?;
            if block.flags & OBJ_GIVEABLE == 0 || perm & !block.flags != 0 {
                return Err(ERROR_ACCESS_DENIED);
            }
            if pid == me { return Ok(()); }
            if pid == 0 || !live.contains(&pid) { return Err(ERROR_INVALID_PROCID); }
            t.hold_as(offset, pid, perm & PAG_WRITE != 0).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)
        }).ok()?;
        Some(r.err().unwrap_or(NO_ERROR))
    }

    /// A page fault at `addr` in the arena from guest code (a write when
    /// `write`; ADR-0049 addendum): a block this process holds comes into
    /// its tables with the access it holds it with, and the instruction
    /// runs again; anything else is an access violation. None: not in the
    /// arena, or no session table.
    pub(crate) fn arena_fault(&self, addr: u32, write: bool) -> Option<super::vmem::FaultKind> {
        use super::vmem::FaultKind;
        if !in_arena(addr) { return None; }
        let me = self.shared.process_id();
        let offset = addr - SHARED_ARENA_BASE;
        let held = self.shared.with_arena_table(|t, _, _| {
            t.containing(offset).map(|b| (b.offset, b.size, b.access_of(me)))
        }).ok()?;
        let Some((base, size, access)) = held.filter(|&(_, _, a)| a != PageAccess::None) else {
            return Some(FaultKind::Violation);
        };
        let present = self.guest_read::<u32>(process_pte_at(addr)?).is_some_and(|e| e & 1 != 0);
        if present && (!write || access == PageAccess::ReadWrite) {
            // Mapped meanwhile by another thread, or a stale TLB entry.
            return Some(FaultKind::Retry);
        }
        if write && access != PageAccess::ReadWrite {
            return Some(FaultKind::Violation);
        }
        for page in (base..base + size).step_by(PAGE as usize) {
            let a = SHARED_ARENA_BASE + page;
            if let Some(at) = process_pte_at(a) {
                self.system_write::<u32>(at, process_pte(a, access));
            }
        }
        Some(FaultKind::Retry)
    }

    /// `DosQueryMem` in the arena (ADR-0049 addendum): a block this
    /// process holds is committed shared memory with the access it holds it
    /// with, to the block's end (`PAG_BASE` on its first page); any other
    /// page is free to it (unverified). None: not in the arena, or no
    /// session table.
    pub(crate) fn arena_query(&self, pb: u32, cb: u32) -> Option<(u32, u32)> {
        if !in_arena(pb) || self.shared.arena.get().is_none() { return None; }
        let me = self.shared.process_id();
        let page = (pb - SHARED_ARENA_BASE) & !(PAGE - 1);
        let held = self.shared.with_arena_table(|t, _, _| {
            t.containing(page).map(|b| (b.offset, b.end(), b.flags, b.access_of(me)))
        }).ok()?;
        let limit = cb.div_ceil(PAGE).max(1).saturating_mul(PAGE);
        Some(match held {
            Some((base, end, flags, access)) if access != PageAccess::None => {
                let write = if access == PageAccess::ReadWrite { PAG_WRITE } else { 0 };
                let first = if page == base { PAG_BASE } else { 0 };
                ((end - page).min(limit), PAG_COMMIT | PAG_SHARED | PAG_READ | write | (flags & PAG_EXECUTE) | first)
            }
            _ => (PAGE, PAG_FREE),
        })
    }

    /// `DosFreeMem` of an arena block: the caller lets it go; it is gone
    /// once no process holds it.
    pub(super) fn arena_free(&self, pb: u32) -> Option<u32> {
        let me = self.shared.process_id();
        let offset = pb.wrapping_sub(SHARED_ARENA_BASE);
        let r = self.shared.with_arena_table(|t, _, freed| {
            let size = t.at(offset).map(|b| b.size);
            match t.release(offset, me) {
                None => Err(ERROR_INVALID_ADDRESS),
                Some(gone) => { freed.extend(gone); Ok(size.unwrap_or(0)) }
            }
        }).ok()?;
        Some(match r {
            Ok(size) => { self.shared.suballoc.lock_or_recover().forget_range(pb, size); NO_ERROR }
            Err(e) => e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn live(pids: &[u32]) -> HashSet<u32> { pids.iter().copied().collect() }

    #[test]
    fn blocks_take_the_lowest_free_pages() {
        let mut t = ArenaTable::default();
        assert_eq!(t.alloc(1, PAG_READ, None, 1), Some(0));
        assert_eq!(t.alloc(4097, PAG_READ, None, 1), Some(0x1000));
        assert_eq!(t.alloc(4096, PAG_READ, None, 2), Some(0x3000));
        assert_eq!(t.blocks()[1].size, 0x2000, "rounded up to pages");
        assert_eq!(t.release(0x1000, 1), Some(Some((0x1000, 0x2000))));
        assert_eq!(t.alloc(4096, PAG_READ, None, 1), Some(0x1000), "first fit reuses the gap");
        assert_eq!(t.alloc(SHARED_ARENA_SIZE, PAG_READ, None, 1), None, "no room");
        assert_eq!(t.alloc(0, PAG_READ, None, 1), None);
        assert_eq!(t.alloc(u32::MAX, PAG_READ, None, 1), None);
        assert_eq!(t.blocks()[0].addr(), SHARED_ARENA_BASE);
    }

    #[test]
    fn holders_come_and_go() {
        let mut t = ArenaTable::default();
        let o = t.alloc(100, PAG_READ | OBJ_GIVEABLE, None, 5).unwrap();
        assert_eq!(t.hold(o, 7), Ok(()));
        assert_eq!(t.hold(o, 7), Ok(()), "holding twice counts once");
        assert_eq!(t.hold(0x5000, 7), Err(HoldError::NoBlock));
        assert_eq!(t.at(o).unwrap().holders, [5, 7]);
        assert_eq!(t.release(o, 9), None, "not a holder");
        assert_eq!(t.release(o, 5), Some(None));
        assert_eq!(t.release(o, 7), Some(Some((0, 0x1000))), "the last one out frees it");
        assert!(t.blocks().is_empty());
        let mut t = ArenaTable::default();
        let o = t.alloc(1, PAG_READ, None, 1).unwrap();
        for pid in 2..=MAX_HOLDERS as u32 { t.hold(o, pid).unwrap(); }
        assert_eq!(t.hold(o, 999), Err(HoldError::Full));
    }

    /// A holder's access (ADR-0049 addendum): the block's, or read only
    /// when it got the block without `PAG_WRITE`; asking again keeps the
    /// larger; a read-only block is read-only to all; the table keeps it.
    #[test]
    fn holders_have_their_own_access() {
        let mut t = ArenaTable { arena: Some((1, 2)), blocks: Vec::new() };
        let o = t.alloc(1, PAG_READ | PAG_WRITE | OBJ_GETTABLE, None, 5).unwrap();
        assert_eq!(t.at(o).unwrap().access_of(5), PageAccess::ReadWrite, "the allocator");
        assert_eq!(t.at(o).unwrap().access_of(6), PageAccess::None, "not a holder");
        t.hold_as(o, 6, false).unwrap();
        assert_eq!(t.at(o).unwrap().access_of(6), PageAccess::Read);
        let text = String::from_utf8(t.encode()).unwrap();
        assert!(text.ends_with(" - 5,6r\n"), "{text}");
        assert_eq!(ArenaTable::parse(text.as_bytes()), t, "read-only holders round-trip");
        t.hold_as(o, 6, true).unwrap();
        assert_eq!(t.at(o).unwrap().access_of(6), PageAccess::ReadWrite, "asking again for more");
        t.hold_as(o, 6, false).unwrap();
        assert_eq!(t.at(o).unwrap().access_of(6), PageAccess::ReadWrite, "and less keeps more");
        t.hold_as(o, 7, false).unwrap();
        assert_eq!(t.release(o, 7), Some(None));
        assert!(t.at(o).unwrap().read_only.is_empty(), "a holder leaving leaves no read-only mark");
        let r = t.alloc(1, PAG_READ, None, 5).unwrap();
        assert_eq!(t.at(r).unwrap().access_of(5), PageAccess::Read, "a read-only block");
        assert_eq!(t.containing(r + 0xFFF).map(|b| b.offset), Some(r));
        assert_eq!(t.containing(r + 0x1000).map(|b| b.offset), None);
        assert!(ArenaTable::parse(b"warpine-shmem 2 1 2\n0 1000 3 - 5,x6r\n").blocks().is_empty(), "bad holder");
    }

    /// The blocks a process stops holding are the ones whose pages leave
    /// its tables: let go, or gone; not those it still holds, nor others'.
    #[test]
    fn lost_blocks_are_those_let_go() {
        let mut t = ArenaTable::default();
        let a = t.alloc(1, PAG_READ, None, 1).unwrap();
        let b = t.alloc(0x2000, PAG_READ, None, 1).unwrap();
        let c = t.alloc(1, PAG_READ, None, 2).unwrap();
        t.hold(b, 2).unwrap();
        let before = t.clone();
        t.release(a, 1);
        t.release(b, 1);
        t.release(c, 2);
        assert_eq!(lost_blocks(&before, &t, 1), [(a, 0x1000), (b, 0x2000)]);
        assert_eq!(lost_blocks(&before, &t, 2), [(c, 0x1000)]);
        assert!(lost_blocks(&before, &before, 1).is_empty());
    }

    #[test]
    fn dead_processes_lose_their_blocks() {
        let mut t = ArenaTable::default();
        let a = t.alloc(1, PAG_READ, Some("\\SHAREMEM\\A".into()), 1).unwrap();
        let b = t.alloc(1, PAG_READ, None, 2).unwrap();
        t.hold(b, 3).unwrap();
        assert_eq!(t.sweep(&live(&[3])), [(a, 0x1000)]);
        assert_eq!(t.at(b).unwrap().holders, [3]);
        assert!(t.by_name("\\SHAREMEM\\A").is_none(), "the name is free again");
        assert_eq!(t.release_all(3), [(b, 0x1000)]);
        assert!(t.blocks().is_empty());
    }

    #[test]
    fn tables_round_trip_and_reject_junk() {
        let mut t = ArenaTable { arena: Some((7, 42)), blocks: Vec::new() };
        t.alloc(10, PAG_READ | PAG_WRITE | OBJ_GETTABLE, Some("\\SHAREMEM\\ÄB C".into()), 3).unwrap();
        t.alloc(5000, PAG_READ, None, 4).unwrap();
        t.hold(0, 9).unwrap();
        let bytes = t.encode();
        assert_eq!(ArenaTable::parse(&bytes), t);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("warpine-shmem 2 7 42\n"));
        let n = hex_name("\\SHAREMEM\\X");
        let junk = format!("warpine-shmem 2 7 42\n\
            0 1000 3 - 1\n\
            800 1000 3 - 1\n\
            1000 1000 3 {n} 1\n\
            2000 1000 3 {n} 2\n\
            3000 1000 3 - 0\n\
            4000 1000 3 - x\n\
            5000 0 3 - 1\n\
            6000 1000 3 - 1 extra\n\
            7000 1000 80000000 - 1\n\
            {:x} 1000 3 - 1\n\
            8000 1000 3 41 1\n\
            9000 1000 3 - 99999999\n\
            a00 1000 3 - 1\n", SHARED_ARENA_SIZE);
        let t = ArenaTable::parse(junk.as_bytes());
        let offsets: Vec<u32> = t.blocks().iter().map(|b| b.offset).collect();
        assert_eq!(offsets, [0, 0x1000], "overlaps, duplicate names, bad holders, flags, ranges and names dropped");
        assert_eq!(ArenaTable::parse(b"other 1\n0 1000 3 - 1\n"), ArenaTable::default());
        assert_eq!(ArenaTable::parse(b"\xff\xfe"), ArenaTable::default());
        assert_eq!(ArenaTable::parse(b"warpine-shmem 2\n").arena, None);
        assert!(ArenaTable::parse(b"warpine-shmem 1 7 42\n0 1000 3 - 1\n").blocks().is_empty(), "an older table");
    }

    #[test]
    fn names_are_canonical_shared_memory_names() {
        assert_eq!(canonical_name("\\sharemem\\a/b").as_deref(), Some("\\SHAREMEM\\A\\B"));
        assert_eq!(canonical_name("/SHAREMEM/X").as_deref(), Some("\\SHAREMEM\\X"));
        for bad in ["\\SHAREMEM\\", "\\PIPE\\X", "SHAREMEM\\X", "\\SHAREMEM\\a\nb"] {
            assert_eq!(canonical_name(bad), None, "{bad:?}");
        }
        let long = format!("\\SHAREMEM\\{}", "A".repeat(MAX_NAME_BYTES));
        assert_eq!(canonical_name(&long), None);
        assert_eq!(unhex_name("4"), None);
        assert_eq!(unhex_name("zz"), None);
    }

    /// A mock loader in a session at `dir` with an arena (not mapped: the
    /// mock's guest memory ends below it).
    fn loader_in_session(dir: &std::path::Path) -> super::super::Loader {
        use super::super::session_threads::{ThreadSession, ThreadSlots, SessionDir, Identity, proc_start_time};
        let l = super::super::Loader::new_mock();
        let pid = std::process::id();
        let me = Identity { pid, start: proc_start_time(pid).unwrap() };
        let mut s = ThreadSession::new(SessionDir::at(dir.join("s")), 64, me, Arc::new(proc_start_time))
            .with_os2pid(l.shared.process_id());
        s.join(None).unwrap();
        *l.shared.thread_slots.lock_or_recover() = ThreadSlots::Session(Box::new(s));
        l.shared.arena.set(Arena::create().unwrap()).unwrap();
        l
    }

    #[test]
    fn api_calls_keep_the_session_table() {
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path());
        let ppb = DYNAMIC_ALLOC_BASE;
        let name = Some("\\SHAREMEM\\T".to_string());
        assert_eq!(l.arena_alloc(ppb, name.clone(), 100, PAG_READ | OBJ_GIVEABLE), Some(NO_ERROR));
        let a = l.guest_read::<u32>(ppb).unwrap();
        assert_eq!(a, SHARED_ARENA_BASE);
        assert_eq!(l.arena_alloc(ppb, name, 100, PAG_READ), Some(ERROR_ALREADY_EXISTS));
        assert_eq!(l.arena_alloc(0x7FFF_FFF0, None, 100, PAG_READ), Some(ERROR_INVALID_PARAMETER));
        assert_eq!(l.arena_get_named(ppb, "\\SHAREMEM\\NONE", PAG_READ | PAG_WRITE), Some(ERROR_FILE_NOT_FOUND));
        assert_eq!(l.arena_get_named(ppb + 4, "\\SHAREMEM\\T", PAG_READ | PAG_WRITE), Some(NO_ERROR));
        assert_eq!(l.guest_read::<u32>(ppb + 4), Some(a));
        assert_eq!(l.arena_get(a, PAG_READ), Some(ERROR_ACCESS_DENIED), "not gettable");
        assert_eq!(l.arena_get(a + 4096, PAG_READ), Some(ERROR_INVALID_ADDRESS));
        assert_eq!(l.arena_give(a, 0x7777, PAG_READ), Some(ERROR_INVALID_PROCID));
        assert_eq!(l.arena_give(a, l.shared.process_id(), PAG_READ), Some(NO_ERROR));
        assert_eq!(l.arena_give(a, 0x7777, PAG_WRITE), Some(ERROR_ACCESS_DENIED), "more than allowed");
        let table = std::fs::read(d.path().join("s").join(SHARED_MEM_TABLE)).unwrap();
        let t = ArenaTable::parse(&table);
        assert_eq!(t.arena, Some(l.shared.arena.get().unwrap().id()));
        assert_eq!(t.blocks().len(), 1);
        assert_eq!(l.arena_free(a), Some(NO_ERROR));
        assert_eq!(l.arena_free(a), Some(ERROR_INVALID_ADDRESS));
        assert_eq!(l.arena_get_named(ppb, "\\SHAREMEM\\T", PAG_READ | PAG_WRITE), Some(ERROR_FILE_NOT_FOUND));
        // A block of a process that ended is let go.
        assert_eq!(l.arena_alloc(ppb, None, 1, PAG_READ), Some(NO_ERROR));
        l.shared.release_shared_arena();
        let t = ArenaTable::parse(&std::fs::read(d.path().join("s").join(SHARED_MEM_TABLE)).unwrap());
        assert!(t.blocks().is_empty());
        l.shared.leave_thread_session();
    }

    /// API calls reach only held blocks (ADR-0049 addendum): the guard
    /// knows this process's blocks after each change, refuses another
    /// process's block, a write to a read-only one and a block let go, and
    /// finds a block given meanwhile by reading the table again.
    #[test]
    fn host_accesses_reach_only_held_blocks() {
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path());
        let me = l.shared.process_id();
        let id = l.shared.arena.get().unwrap().id();
        let path = d.path().join("s").join(SHARED_MEM_TABLE);
        let p = path.clone();
        l.shared.guest_mem.guard_arena(ArenaGuard { holds: Vec::new(), refresh: Box::new(move || held_in_table_file(&p, id, me)) });
        let gm = &l.shared.guest_mem;
        assert!(!gm.arena_allows(SHARED_ARENA_BASE, 4, false), "nothing held yet");
        assert!(gm.arena_allows(SHARED_ARENA_BASE - 4, 4, true), "below the arena");
        assert_eq!(l.arena_alloc(DYNAMIC_ALLOC_BASE, None, 0x2000, PAG_READ | PAG_WRITE), Some(NO_ERROR));
        let a = l.guest_read::<u32>(DYNAMIC_ALLOC_BASE).unwrap();
        assert!(gm.arena_allows(a, 0x2000, true), "its own block, whole");
        assert!(!gm.arena_allows(a + 0x1FFE, 4, false), "not past its end");
        // Another process's block, then given read-only (written by hand,
        // as that process would): found on the next look.
        let mut t = ArenaTable::parse(&std::fs::read(&path).unwrap());
        let other = t.alloc(1, PAG_READ | PAG_WRITE, None, 0x7777).unwrap();
        std::fs::write(&path, t.encode()).unwrap();
        let b = SHARED_ARENA_BASE + other;
        assert!(!gm.arena_allows(b, 4, false), "another process's block");
        t.hold_as(other, me, false).unwrap();
        std::fs::write(&path, t.encode()).unwrap();
        assert!(gm.arena_allows(b, 4, false), "given: read");
        assert!(!gm.arena_allows(b, 4, true), "but not written");
        assert_eq!(l.arena_free(a), Some(NO_ERROR));
        assert!(!gm.arena_allows(a, 4, false), "let go");
        l.shared.leave_thread_session();
    }

    /// Reaping a child sweeps the table at once (ADR-0049 addendum): a
    /// block only a dead process held is gone from the file, without
    /// waiting for the next call on the table; a live holder stays.
    #[test]
    fn reaping_a_child_sweeps_the_table_at_once() {
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path());
        let me = l.shared.process_id();
        let path = d.path().join("s").join(SHARED_MEM_TABLE);
        let mut t = ArenaTable { arena: Some(l.shared.arena.get().unwrap().id()), blocks: Vec::new() };
        let dead = t.alloc(1, PAG_READ, Some("\\SHAREMEM\\DEAD".into()), 0x7777).unwrap();
        let shared = t.alloc(1, PAG_READ, None, 0x7777).unwrap();
        t.hold(shared, me).unwrap();
        std::fs::write(&path, t.encode()).unwrap();
        l.shared.sweep_after_child();
        let t = ArenaTable::parse(&std::fs::read(&path).unwrap());
        assert!(t.at(dead).is_none() && t.by_name("\\SHAREMEM\\DEAD").is_none(), "the dead process's block is gone");
        assert_eq!(t.at(shared).map(|b| b.holders.clone()), Some(vec![me]), "the live holder keeps its block");
        l.shared.leave_thread_session();
    }

    #[test]
    fn a_table_of_another_arena_is_not_shared() {
        let d = tempfile::tempdir().unwrap();
        let l = loader_in_session(d.path());
        // A live process (this one, by its OS/2 id) holds a block of another arena.
        let mut other = ArenaTable { arena: Some((1, 2)), blocks: Vec::new() };
        other.alloc(1, PAG_READ, None, l.shared.process_id()).unwrap();
        std::fs::write(d.path().join("s").join(SHARED_MEM_TABLE), other.encode()).unwrap();
        assert_eq!(l.arena_alloc(DYNAMIC_ALLOC_BASE, None, 1, PAG_READ), None, "falls back to the process");
        // Once its blocks are gone, the table is taken over.
        let mut dead = ArenaTable { arena: Some((1, 2)), blocks: Vec::new() };
        dead.alloc(1, PAG_READ, None, 4242).unwrap();
        std::fs::write(d.path().join("s").join(SHARED_MEM_TABLE), dead.encode()).unwrap();
        assert_eq!(l.arena_alloc(DYNAMIC_ALLOC_BASE, None, 1, PAG_READ), Some(NO_ERROR));
        l.shared.leave_thread_session();
    }

    #[test]
    fn the_arena_is_shared_memory_mapped_into_guest_memory() {
        let arena = Arena::create().unwrap();
        // Two guests (as two processes would have) see the same pages.
        let a = super::super::guest_mem::GuestMemory::alloc(2 * SHARED_ARENA_SIZE as usize);
        let b = super::super::guest_mem::GuestMemory::alloc(2 * SHARED_ARENA_SIZE as usize);
        a.map_shared(SHARED_ARENA_SIZE, SHARED_ARENA_SIZE as usize, arena.fd()).unwrap();
        b.map_shared(SHARED_ARENA_SIZE, SHARED_ARENA_SIZE as usize, arena.fd()).unwrap();
        a.write::<u32>(SHARED_ARENA_SIZE + 0x2004, 0xC0FFEE).unwrap();
        assert_eq!(b.read::<u32>(SHARED_ARENA_SIZE + 0x2004), Some(0xC0FFEE));
        a.write::<u32>(0x2004, 1).unwrap();
        assert_eq!(b.read::<u32>(0x2004), Some(0), "private pages stay private");
        arena.zero(0x2000, 0x1000);
        assert_eq!(a.read::<u32>(SHARED_ARENA_SIZE + 0x2004), Some(0), "a freed block reads zero");
        // Bounds and alignment are checked.
        assert!(a.map_shared(1, 4096, arena.fd()).is_err());
        assert!(a.map_shared(0, 3 * SHARED_ARENA_SIZE as usize, arena.fd()).is_err());
        assert!(a.map_shared(0, 0, arena.fd()).is_err());
        // The arena cannot be resized.
        let f = std::fs::File::from(arena.fd.try_clone().unwrap());
        assert!(f.set_len(4096).is_err());
        assert!(Arena::checked(arena.fd.try_clone().unwrap()).is_ok());
        let plain = OwnedFd::from(tempfile::tempfile().unwrap());
        assert!(Arena::checked(plain).is_err(), "an unsealed file is no arena");
    }
}
