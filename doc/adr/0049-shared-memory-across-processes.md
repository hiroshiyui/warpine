# ADR-0049: Shared memory across processes: a sealed memfd arena at the top of guest memory, a table in the session directory

- **Status:** Accepted; extends ADR-0020, ADR-0022 and ADR-0032
- **Date:** 2026-11-19
- **Sources:** `src/loader/shared_arena.rs`, `src/loader/session_threads.rs` (`SessionTables`, the OS/2 process id in records), `src/loader/guest_mem.rs` (`map_shared`), `src/loader/inherit.rs` (`admit_child`), `src/loader/stubs.rs`, `src/loader/doscalls.rs` (`DosFreeMem`); `samples/shmem_test`; IBM *Control Program Programming Reference* (`DosAllocSharedMem`, `DosGetNamedSharedMem`, `DosGetSharedMem`, `DosGiveSharedMem`, `DosFreeMem`), as on EDM/2

## Context

Every OS/2 process runs in a Warpine host process of its own, with its own KVM guest memory (ADR-0020). Until now shared memory was shared only by the threads of one process:
- giving a block to a child failed with `ERROR_ACCESS_DENIED`;
- a child never found its parent's `\SHAREMEM\` names.

Programs that pass data between processes this way did not work, among them 4OS2 piping into its internal commands.

OS/2 places a shared object at the same linear address in every process that uses it ("the returned address will be the same as the one returned to the process that created the memory block"). Warpine guests run flat, without paging, so guest physical addresses are linear addresses.

## Decision

- **The shared arena.** The top `SHARED_ARENA_SIZE` (32 MB) of every process's 256 MB guest memory, from `SHARED_ARENA_BASE` (`0x0E000000`), is reserved for shared memory. Private allocations end below it.
  - The first process of a session creates a `memfd`. It is sealed against shrinking, growing and further sealing (`F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`), so no host access through the mapping can fault past its end.
  - `GuestMemory::map_shared` maps the memfd over those guest pages with `MAP_SHARED | MAP_FIXED`. The pages stay inside the one KVM memory slot and inside the bounds every guest-memory access already checks, so no new slot is needed and no accessor changes.
- **Children inherit the arena.** `admit_child`, the gate of `DosExecPgm`, `WinStartApp` and CMD.EXE's external commands, passes the descriptor number in `WARPINE_ARENA_FD`. As for pipes (ADR-0022), close-on-exec is cleared only in the forked child.
  - The child uses the descriptor only if it is a sealed memfd of exactly the arena's size.
  - Without the variable, the process creates a new arena. With a malformed variable or an unusable descriptor, the process shares nothing.
- **The table.** Blocks and their holders are recorded in `shmem` in the session's private thread-accounting directory (ADR-0032), read and written under the directory's `flock`. It is written aside and renamed into place, so a process killed mid-write leaves the old table.
  - Each line is a block: offset, size, flags, the hex-encoded upper-case name, and the holders' OS/2 process ids.
  - The header names the arena by device and inode. A process with another arena takes the table over only when every block in it is gone; otherwise it keeps shared memory to itself.
  - The table is not in guest memory. It is parsed as hostile input: strict fields; at most 1024 blocks, 64 holders per block and 255-byte names; blocks inside the arena, page-aligned, without overlaps or duplicate names. Malformed lines are dropped.
- **Liveness by OS/2 process id.** Session records now carry the OS/2 process id: a running process's own, and in a reservation the child's.
  - A holder counts while a live record names it. A child being started therefore counts as soon as `DosExecPgm` returns its id, and a parent can give it a block before it runs.
  - Every use of the table first drops dead holders. A process killed by a signal loses its blocks the next time any process of the session uses the table, and its own exit releases them at once (`leave_thread_session`).
- **Memory of blocks.** A block without holders is gone: its pages are punched out of the memfd (`FALLOC_FL_PUNCH_HOLE`) while the table is still locked, so they read zero in every process and their memory goes back to the host. A new block is punched as well before its address is returned. Blocks are allocated first-fit in pages.
- **The calls.**
  - `DosAllocSharedMem` creates a block held by the caller.
  - `DosGetNamedSharedMem` (access flags checked, `ERROR_INVALID_NAME` outside `\SHAREMEM\`) and `DosGetSharedMem` (the block must be `OBJ_GETTABLE`) add the caller as a holder.
  - `DosGiveSharedMem` adds a live process of the session, which must be the caller's own block, `OBJ_GIVEABLE`, with no more access than it was allocated with.
  - `DosFreeMem` removes the caller. A second `DosFreeMem` is `ERROR_INVALID_ADDRESS`. Getting a block twice counts once (unverified).
- **Without a session** (unit-test loaders, an unusable session directory), shared memory stays within the process, as before.

## Consequences

- Named, gettable and giveable shared memory works between all processes of a session, at one address in each.
- Each process has 32 MB less private memory: 192 MB from `DYNAMIC_ALLOC_BASE` instead of 224 MB. `QSV_MAXSHMEM` reports the arena's size.
- The memfd is sparse, so only pages in use take memory. The kernel frees it when the last process of the session ends.
- Security:
  - The arena holds only what guests put there, and the host reads it only through the bounds-checked accessors, as with any guest memory. Its fixed size rules out `SIGBUS` on host accesses.
  - The table and descriptor live in the user's mode-0700 session directory and in inherited descriptors, so other users cannot reach them.
- Limits:
  - There is no paging, so a guest can read and write every block in the arena whether or not its process holds it.
  - Access flags of a get or give are checked but not enforced.
  - A killed process's blocks stay until the table is next used.
  - OS/2 process ids wrap after 0xFFFF, so a dead holder whose id was reused before the next sweep would count as alive.
  - 16-bit shared segments (`DosAllocShrSeg`) do not exist yet.
- Unverified:
  - holding a block twice counting once;
  - `ERROR_INVALID_ADDRESS` from `DosGetSharedMem` for an address that is not a block;
  - `ERROR_NOT_ENOUGH_MEMORY` when a block has `MAX_HOLDERS`.

## Addendum (2026-11-24): access enforced by paging; a reaped child's blocks swept at once

Guests now run paged (ADR-0088, ADR-0113, ADR-0114), so the limits above that came from flat memory can go.

- **The arena goes through process tables.** Its 32 MB get eight page tables after the allocator's (`paging.rs`: `SHARED_TABLES`, `SHARED_CHUNKS`), in the supervisor-only paging arena; the directory entries of its chunks lead to them instead of large pages. Every page starts not present. The paging arena loses two thread blocks to them: 736 threads a process (was 738, ADR-0115).
- **A block comes in when touched.** A page fault in the arena (`Loader::arena_fault`) looks the address up in the session's table: a block this process holds is written into its tables with the access it holds it with, and the instruction runs again; a block it does not hold, or a write to one it holds read-only, is `XCPT_ACCESS_VIOLATION`. Blocks given to the process by another one need no notice: they come in at the first touch.
- **A block goes out when let go.** `with_arena_table` compares the process's holds before and after each change (`lost_blocks`); the pages of every block it no longer holds leave its tables and every vCPU's TLB (`unmap_arena_blocks`, the TLB epoch as in ADR-0114).
- **Access per holder.** A holder's line in the table is its OS/2 id, followed by `r` when it may only read (`warpine-shmem 2`; an older table is not read). The allocator gets the block's access; `DosGetNamedSharedMem` / `DosGetSharedMem` read-only unless their flags have `PAG_WRITE`, `DosGiveSharedMem` likewise for the receiver; a block allocated without `PAG_WRITE` is read-only to everyone. Asking again keeps the larger access.
- **`DosQueryMem` in the arena** reports a block this process holds as `PAG_SHARED | PAG_COMMIT | PAG_READ` (`PAG_WRITE` when it may write, `PAG_EXECUTE` when the block has it), to the block's end, `PAG_BASE` on its first page; any other arena page is `PAG_FREE` to it.
- **Reaping a child sweeps.** Every path that reaps a child (`SharedState::child_exit_code`) sweeps the table at once, so a child killed before it could let its blocks go leaves nothing behind once its parent has seen it end. A process whose parent never waits for it is still swept at the next use of the table.

Remaining limits:
- ~~The host side does not check holds~~ and ~~16-bit programs reach all of the arena~~: see the second addendum below.
- `PAG_EXECUTE` alone reads (32-bit paging has no execute-only pages).

Unverified (Warpine's own): read-only unless `PAG_WRITE` is asked for on a get or give; asking again keeping the larger access; `PAG_FREE` for an arena page the process does not hold.

Tests: `holders_have_their_own_access`, `lost_blocks_are_those_let_go`, `reaping_a_child_sweeps_the_table_at_once`, `the_process_tables_map_the_allocators_region`, `no_entry_maps_a_page_elsewhere_than_itself_but_the_thread_local_page`; `samples/shmem_test` (a gettable block not got yet faults and is free to `DosQueryMem`, a block got read-only cannot be written, a block let go cannot be touched; 31 checks + 18 in the child).

## Addendum (2026-11-25): API calls held to the same holds; 16-bit programs checked

- **API calls.** `GuestMemory` keeps the blocks the process holds (`ArenaGuard`: guest start and end, writable or not), set after every change of the table (`with_arena_table` → `held_blocks`). A read or write through a guest address that touches the arena (`read`, `read_bytes`, `write`, `write_bytes`, `slice_mut`, `Loader::guest_ptr`) must lie inside held blocks, writable ones for a write; else it fails like any bad address and the call returns its usual error (`ERROR_INVALID_PARAMETER` for `DosRead` / `DosWrite`). A miss first reads the session's table again (`held_in_table_file`), without its lock: the table is replaced by rename, so the read sees a whole version and no lock of the session can deadlock with the caller's. That finds a block another process has just given. Warpine's own writes to its structures (`write_system`) are not checked.
- **Read-only buffers.** Calls that only read a guest buffer read it with `Loader::guest_read_buffer` (32-bit and 16-bit `DosWrite`, 16-bit `VioWrtTTY`, `DosWriteQueue`), not `guest_slice_mut`, so a block held read-only can be written to a file; Warpine's protected memory stays refused there, as before.
- **16-bit programs.** Their threads get their paging block from the same `alloc_paging_block`, whose directory leads the arena's chunks to the process tables: their code reaches no arena page, as they can hold no block (no 16-bit call allocates one). The note above was wrong; `samples/ne_arena_test` reads the arena through the GDT data tile that covers it and traps.
- **Fixed on the way:** `DosRead` of a file ignored a buffer it could not write and returned `NO_ERROR`; it now returns `ERROR_INVALID_PARAMETER`, as for a pipe.

Tests: `the_arena_guard_allows_only_held_ranges`, `host_accesses_reach_only_held_blocks`; `shmem_test` (`DosWrite` from a block not got fails, from one got read-only works, `DosRead` into it fails; 31 checks + 22 in the child); `a_16_bit_program_cannot_reach_the_shared_arena`.
