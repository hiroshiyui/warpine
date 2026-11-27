# ADR-0114: Page states of allocated memory, held by the page tables

- **Status:** Accepted (phase 1: the memory calls and guard pages; phase 2: sparse thread stacks; phase 3: read-only objects; phase 4: free memory not present); its tables' format and place superseded by ADR-0131 (64-bit entries, the process tables in their own large page, execute permission)
- **Date:** 2026-11-22
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosAllocMem`, `DosSetMem`, `DosQueryMem`, `DosFreeMem`, `DosCreateThread` (`STACK_SPARSE`, `STACK_COMMITTED`), and the exceptions `XCPT_GUARD_PAGE_VIOLATION`, `XCPT_ACCESS_VIOLATION`, `XCPT_UNABLE_TO_GROW_STACK`.
  - The OS/2 headers as Open Watcom ships them: `bsememf.h` (`PAG_*`, `fPERM`, `fSET`), `bsedos.h` (`STACK_SPARSE`), `bsexcpt.h` (the exception codes and their `ExceptionInfo`).
  - Intel SDM vol. 3: 32-bit paging entries (4.3), the TLB and its invalidation (4.10).
  - The KVM API: `KVM_SET_SREGS`. A CR3 that differs from the vCPU's current one makes KVM reload the vCPU's paging, its TLB with it. This was checked by the sample below, not read from KVM's source.

## Context

Warpine's memory was flat:
- `DosAllocMem` ignored its flags and committed every page read-write;
- `DosSetMem` did nothing;
- `DosQueryMem` returned made-up values.

Every allocator page was a user-writable 4 MB large page in every thread's tables. So:
- a program that reserves memory and commits it as it goes saw no difference;
- one that relies on read-only pages, decommitting, or guard pages got none of them.

Guard pages matter most: OS/2 grows thread stacks with them (`STACK_SPARSE` is the default), and runtimes use them to commit memory on demand.

ADR-0088 made 32-bit threads run paged, and ADR-0113 made them run at ring 3 with Warpine's own pages supervisor-only and CR0.WP set. The page tables can now carry these states.

## Decision

1. **Process tables for the allocator's region.** The 48 page tables mapping `DYNAMIC_ALLOC_BASE`..`SHARED_ARENA_BASE` (192 KB) lie at the start of the paging arena (`PROCESS_TABLES`), supervisor-only.
   - Every thread's directory points those chunks at them instead of at large pages, so one write reaches every thread of the process.
   - They start present and writable, as the large pages were (`ensure_process_tables`).
   - The paging blocks follow them (`PAGING_BLOCKS_BASE`). With phase 3, 226 (191 before this ADR; 482 since ADR-0115).
2. **Page states** (`vmem.rs`, `PageMap`): each `DosAllocMem` allocation is a region, with each page's `PAG_*` state and the access it was allocated with (`PAG_DEFAULT`). A page's process-table entry follows its state:
   - committed with `PAG_WRITE`: present and writable;
   - committed with read or execute access only: present and read-only;
   - reserved, or a guard page: not present.
3. **The calls:**
   - **`DosAllocMem`** honours its flags. At least one access bit is required; `PAG_GUARD` needs `PAG_COMMIT`; 0 bytes or an unknown flag is `ERROR_INVALID_PARAMETER`. `PAG_GUARD` makes every page of the allocation a guard page.
   - **`DosSetMem`** commits (zeroed pages), decommits, or changes access and guard (`fSET` and `PAG_GUARD`). Committing a committed page, or decommitting or changing an uncommitted one, is `ERROR_ACCESS_DENIED`; a range past the allocation is `ERROR_INVALID_ADDRESS`.
   - **`DosQueryMem`** returns the run of pages with the same state, at most the size asked for, with `PAG_BASE` on an allocation's first page. An uncommitted page reports the access it was allocated with.
   - **`DosFreeMem`** makes the pages present and writable again.
4. **Faults** (`page_fault_kind`, in the vCPU loop's #PF path):
   - a guard page touched loses its guard and raises `XCPT_GUARD_PAGE_VIOLATION`, continuable, with `ExceptionInfo[0]` the access and `[1]` the address. If no handler takes it, the default action is to go on where it faulted (`continues_by_default`);
   - a page whose state now allows the access (a TLB held an older entry) runs the instruction again;
   - anything else is `XCPT_ACCESS_VIOLATION`, as before.
5. **TLBs.** When a page loses access, through `DosSetMem` or a `DosAllocMem` that restricts pages which were present before, `flush_tlbs` counts `SharedState::tlb_epoch` up and kicks every vCPU.
   - Before it runs guest code again, each vCPU that sees a new epoch toggles CR3's write-through bit through `KVM_SET_SREGS` (`flush_tlb`), which makes KVM drop its translations.
   - CR3 is compared without its low 12 bits (`CR3_ADDRESS`).
6. **What is not tracked** stays present and writable, as before: Warpine's own allocations (TIBs, stacks, PM structures), free pages, shared memory, and the program's objects. `DosSetMem` on such memory is accepted and changes nothing; `DosQueryMem` reports it committed and writable (`PAG_SHARED` in the shared arena).
7. **Phase 2: sparse thread stacks** (`STACK_SPARSE`, the default of `DosCreateThread`; `allocate_stack`, `grow_stack`).
   - A thread's stack at ring 3 is a region of its own: the top page committed, a guard page below it, the rest reserved. `STACK_COMMITTED` commits it whole, and so does a stack of fewer than four pages.
   - **A touch anywhere in the reserved part commits every page from there up to the committed part,** and makes the page below a guard page. This is not only through the guard page, as planned at first, for two reasons: a frame larger than a page would jump over the guard, needing stack probes in every compiler's output; and Warpine itself writes exception and callback frames below a thread's stack pointer, bypassing the page tables, so the guest would later fault reading its own handler's arguments.
   - **Growth happens in Warpine,** without calling the program's handlers, which would need stack themselves.
   - **The bottom two pages** raise `XCPT_UNABLE_TO_GROW_STACK` the first time they are touched, leaving them for the handlers; after that they commit like the rest.
   - **Only at ring 3** (since ADR-0113's step 5 the only ring). A guest at ring 0 takes an exception on its own stack: a fault *on* that stack cannot be delivered, and ends in a triple fault (found by the sample). At ring 0 (`WARPINE_RING=0`) stacks stay committed whole.
   - When a thread ends, its stack's pages are present and writable again.
8. **Phase 3: read-only objects** (`protect_objects`, `read_only_pages`).
   - The program area (0 up to the API thunks) is mapped through four more process tables (`EXE_CHUNKS`), before the allocator's (`ALLOC_TABLES`). The first carries the not-present low 64 KB that each thread's own low table held, so a paging block is four pages now (directory, TLM table, thread-local page, ring-0 page), and the arena holds 226 blocks.
   - Once a module's fixups are in, the pages of its objects without the LX write flag (`OBJWRITE`, 0x0002) become read-only: the program's own objects, its resource objects, and every DLL's. A page such an object shares with a writable object stays writable. Writing them raises `XCPT_ACCESS_VIOLATION`, at ring 0 too (CR0.WP).
   - A DLL's objects are made writable again before their memory is freed (`free_module_objects`, from `DosFreeModule`, the unload `_DLL_InitTerm`'s return and `WinDeleteLibrary`), so the allocator never hands out a read-only page.
   - The process tables are written before the first entry is changed (`apply_page_changes`), since a program's objects are protected before its first thread exists.
9. **Phase 4: free memory is not present** (`sync_allocations`).
   - The allocator's region starts not present. `MemoryManager` records every allocation and free (`take_pending`, with a flag the vCPU loops read without its lock). Before a vCPU runs guest code, the pending changes go into the process tables: an allocated block present and writable, a freed one not present. Pages of `DosAllocMem` regions keep their own states, and a region's pages are not present once it is freed.
   - A page fault on a page that is allocated, and present once the pending changes are in (another thread allocated it just now), runs the instruction again (`allocated_and_present`, only for a not-present fault).
   - A free flushes only the freeing thread's TLB (`take_freed_here`), not the other vCPUs': Warpine frees often (a callback's scratch memory), and kicking every vCPU each time would cost more than a stale entry, which only lets another thread reach memory just freed.
   - A retried fault rewrites the page's entry from its state, so an entry that disagrees with its state cannot make a fault repeat for ever.
   - **`DosQueryMem` on untracked memory** answers from the process tables (`query_untracked`): a page not present is `PAG_FREE`, a read-only page committed and read-only, a writable one committed and writable; the first page of an allocator block has `PAG_BASE`; the run ends at the size asked for, another state, or the next block. Memory the process tables do not map (shared memory, Warpine's own areas) still reads as one committed, writable page, `PAG_SHARED` in the shared arena.

## Warpine's choices (unverified)

- **Free memory faults (phase 4),** but another thread may still reach memory just freed through its TLB, until its next flush.
- **The error codes** for committing committed pages, decommitting reserved ones, or changing uncommitted ones (`ERROR_ACCESS_DENIED`), and `PAG_GUARD` without `PAG_COMMIT` (`ERROR_INVALID_PARAMETER`).
- **`PAG_GUARD` accepted by `DosSetMem`,** although `fSET` does not include it.
- **A guard violation no handler takes goes on** with the guard gone.
- **`DosQueryMem` on an uncommitted page** reports the allocation's access without `PAG_COMMIT`.
- **The TLB flush** relies on KVM reloading a vCPU's paging when CR3 changes, observed, not documented.
- **A stack grows wherever it is touched,** not only through its guard page, and without calling the program's handlers; OS/2 grows it through the guard page (the documentation does not say whether the handlers see `XCPT_GUARD_PAGE_VIOLATION` for a stack's guard page first).
- **`XCPT_UNABLE_TO_GROW_STACK` from the bottom two pages,** once, continuable; with no handler it ends the process (`TC_EXCEPTION`).
- **Stacks of fewer than four pages** are committed whole.
- **The main thread's stack is the LX header's `STACKSIZE` below the entry ESP,** not below the stack object's base (Open Watcom puts the data and the stack in one object); a page it shares with the object's data stays committed; a program whose header has no `STACKSIZE` (0, or a header ending at 0xAC) keeps its stack committed whole.
- **`PAG_EXECUTE` for a module's executable objects only** (the LX `OBJEXEC` flag); OS/2's exact flags for code are not documented.
- **182 threads per process** instead of 191 (ADR-0113's arena, now shared with the process tables).

## Consequences

- Programs get OS/2's page states: reserved memory, read-only pages, decommitting and guard pages behave as documented. A program that used reserved memory without committing it, which OS/2 would have stopped, now faults too.
- Every thread of a process shares the allocator's page tables; a change costs one entry write, plus a TLB flush of every vCPU when access is taken away.
- **Tests:**
  - `samples/mem_protect_test`: 33 checks, Open Watcom, GCC and clang, at both rings. They include a read-only page on the thread that wrote it, and on another thread that wrote it.
  - Unit tests: `vmem.rs` (allocation, `DosSetMem` and its errors, `DosQueryMem`, faults) and `paging.rs` (the process tables).
- **Phase 2:** `samples/stack_grow_test` (11 checks at ring 3, 6 at ring 0; Open Watcom, GCC and clang): a sparse stack's reserved bottom, growth by recursion and by a frame of four pages, `STACK_COMMITTED`, and `XCPT_UNABLE_TO_GROW_STACK` left by `longjmp`. Unit test: `a_sparse_stack_grows_down_and_runs_out_once`. Before, a thread that overflowed its stack wrote over the memory below it without a fault.
- **Phase 3:** `mem_protect_test` checks that the program's code can be read but not written (35 checks); unit test `read_only_objects_lose_only_their_own_pages`. The suite passes at both rings and with GCC-built samples, so none of them writes its own code.
- **Phase 4:** `mem_protect_test` checks that memory freed with `DosFreeMem`, and memory never allocated, cannot be read, and that `DosQueryMem` reports freed memory as `PAG_FREE` and the program's code as read-only (40 checks); unit test `query_mem_answers_untracked_memory_from_the_page_tables`; the suite passes at both rings and with GCC-built samples, so neither the samples nor Warpine itself touches the allocator's memory without allocating it.
- **Found on the way:**
  - Phase 3's first run left the program's code writable: its objects were protected before the process tables were first written, which then made every page writable again. `apply_page_changes` now writes the tables first.
  - Phase 4's first version kept phase 1's assumption that an allocator page is present unless a region says otherwise: a sparse stack's top page was never made present, and the thread faulted on it for ever (the retry is self-healing since). Every region now writes all its pages' entries.
  - A DLL object was given its virtual size (at least a page), while all its pages were loaded; an object smaller than its pages had file data written past its block. It now gets its pages' size (`load_dll_impl`).
  - A `DosAllocMem` that restricted pages a freed block had left in a TLB did not fault until `flush_tlbs` was also called there.
  - The `warpine-os2` crate's `set_mem` documentation gave wrong flag values; corrected.
- **Later: the main thread's stack, and `PAG_EXECUTE`.**
  - Thread 1's stack is sparse too (`MainStack`, `sparse_main_stack`): the header's `STACKSIZE` bytes below the entry ESP, made a stack region as other threads' are; its TIB gets the bounds, which were 0. lx-link writes `STACKSIZE` (its header is 0xC4 bytes now, as Open Watcom's). `stack_grow_test` checks the bounds, the reserved bottom, growth and `XCPT_UNABLE_TO_GROW_STACK` on the main thread (18 checks); unit test `the_main_stack_lies_below_the_entry_esp`.
  - `DosQueryMem` on a code object reports `PAG_EXECUTE` (`PageMap::mark_executable`, forgotten when the module is freed). `mem_protect_test` checks code and data (42 checks).

## Addendum (2026-11-25): two races between threads

CI once showed a thread created with `CREATE_SUSPENDED` that never ran after `DosResumeThread` (`thread_control_test`, GCC job; not seen locally in 150 runs). Three races could take away the pages of memory just handed to a thread; two-thread stress tests on the mock (`vmem.rs`) show them, failing in most runs before the fix and in none after:

- **Page states written after the memory was freed.** A thread's end freed its stack in the allocator and only then gave up the stack's region (`free_page_states`), whose committed pages it writes "not present". In between, another thread could be given the same address — `DosCreateThread` for a thread waiting on this one's end — and its new stack lost its pages: it faulted at its first push. `DosFreeMem` had the same order. Both now give up the page states first, while the memory is still allocated (`Loader::release_thread_memory`, also used by `DosCreateThread`'s failure path, and `dos_free_mem`), as `free_module_objects` already did.
- **Pending changes written out of order.** `apply_pending_allocations` took a batch of the allocator's changes under its lock but wrote the page-table entries after releasing it, so two threads could write two batches in either order: a free written after the allocation that followed it unmapped the new block. Taking and writing are now one step under `SharedState::pte_sync` (taken before `mem_mgr` and `vmem`). Taking a batch also clears the "pending" flag before it is written, so a `sync_allocations` in that moment returned at once and its caller — a vCPU about to enter the guest, or the page-fault path asking whether a page was allocated meanwhile — saw the entry not yet written; it now waits for `pte_sync` even when the flag is clear (one uncontended lock per call).

Tests: `a_freed_stack_never_unmaps_a_block_given_out_meanwhile`, `dos_free_mem_never_unmaps_a_block_given_out_meanwhile`.

## Addendum (2026-11-27): TLB shootdowns

The security audit of 2026-11-25 found that a page losing access left a window: `flush_tlbs` counted the TLB epoch up and kicked every vCPU, but returned at once, so another thread of the process, still in guest code, could reach the page through a stale TLB entry after the call had returned, a freed shared-arena block even after another process got it.

`flush_tlbs` is now a shootdown (`vmem::TlbSync`, `SharedState::shoot_down_tlbs`): it counts the epoch up, kicks every vCPU and waits until each one is either out of guest code or clean for the new epoch. Each vCPU marks itself in guest code before it reads the epoch and out after `KVM_RUN`, both sequentially consistent, so a shootdown either is seen by a vCPU entering guest code or waits for it. A kicked vCPU leaves guest code within microseconds; the wait gives up after `SHOOTDOWN_TIMEOUT` (2 s) with a warning rather than hang the calling thread (unverified as a bound: no vCPU has been seen to take it). The shared arena's unmapping shoots down the same way. `mem_protect_test` checks that a thread writing a page all the time faults once `DosSetMem` makes it read-only and that none of its writes lands after the call returns; that window is too short for a guest to hit reliably, so the wait itself is tested in `vmem.rs` (`a_tlb_shootdown_waits_for_every_vcpu_in_guest_code`).
