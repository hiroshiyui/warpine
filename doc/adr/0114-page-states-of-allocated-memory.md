# ADR-0114: Page states of allocated memory, held by the page tables

- **Status:** Accepted (phase 1: the memory calls and guard pages; phase 2: sparse thread stacks; phase 3: read-only objects)
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
   - The paging blocks follow them (`PAGING_BLOCKS_BASE`). With phase 3, 226 (191 before this ADR).
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
   - **Only at ring 3.** A guest at ring 0 takes an exception on its own stack: a fault *on* that stack cannot be delivered, and ends in a triple fault (found by the sample). At ring 0 (`WARPINE_RING=0`) stacks stay committed whole.
   - When a thread ends, its stack's pages are present and writable again.
8. **Phase 3: read-only objects** (`protect_objects`, `read_only_pages`).
   - The program area (0 up to the API thunks) is mapped through four more process tables (`EXE_CHUNKS`), before the allocator's (`ALLOC_TABLES`). The first carries the not-present low 64 KB that each thread's own low table held, so a paging block is four pages now (directory, TLM table, thread-local page, ring-0 page), and the arena holds 226 blocks.
   - Once a module's fixups are in, the pages of its objects without the LX write flag (`OBJWRITE`, 0x0002) become read-only: the program's own objects, its resource objects, and every DLL's. A page such an object shares with a writable object stays writable. Writing them raises `XCPT_ACCESS_VIOLATION`, at ring 0 too (CR0.WP).
   - A DLL's objects are made writable again before their memory is freed (`free_module_objects`, from `DosFreeModule`, the unload `_DLL_InitTerm`'s return and `WinDeleteLibrary`), so the allocator never hands out a read-only page.
   - The process tables are written before the first entry is changed (`apply_page_changes`), since a program's objects are protected before its first thread exists.

## Warpine's choices (unverified)

- **Untracked memory stays reachable:** reading free memory, or a block `DosFreeMem` gave back, does not fault, as on OS/2 it would.
- **The error codes** for committing committed pages, decommitting reserved ones, or changing uncommitted ones (`ERROR_ACCESS_DENIED`), and `PAG_GUARD` without `PAG_COMMIT` (`ERROR_INVALID_PARAMETER`).
- **`PAG_GUARD` accepted by `DosSetMem`,** although `fSET` does not include it.
- **A guard violation no handler takes goes on** with the guard gone.
- **`DosQueryMem` on an uncommitted page** reports the allocation's access without `PAG_COMMIT`.
- **The TLB flush** relies on KVM reloading a vCPU's paging when CR3 changes, observed, not documented.
- **A stack grows wherever it is touched,** not only through its guard page, and without calling the program's handlers; OS/2 grows it through the guard page (the documentation does not say whether the handlers see `XCPT_GUARD_PAGE_VIOLATION` for a stack's guard page first).
- **`XCPT_UNABLE_TO_GROW_STACK` from the bottom two pages,** once, continuable; with no handler it ends the process (`TC_EXCEPTION`).
- **Stacks of fewer than four pages,** and the main thread's stack (the program's stack object), are committed whole.
- **182 threads per process** instead of 191 (ADR-0113's arena, now shared with the process tables).

## Consequences

- Programs get OS/2's page states: reserved memory, read-only pages, decommitting and guard pages behave as documented. A program that used reserved memory without committing it, which OS/2 would have stopped, now faults too.
- Every thread of a process shares the allocator's page tables; a change costs one entry write, plus a TLB flush of every vCPU when access is taken away.
- **Tests:**
  - `samples/mem_protect_test`: 33 checks, Open Watcom, GCC and clang, at both rings. They include a read-only page on the thread that wrote it, and on another thread that wrote it.
  - Unit tests: `vmem.rs` (allocation, `DosSetMem` and its errors, `DosQueryMem`, faults) and `paging.rs` (the process tables).
- **Phase 2:** `samples/stack_grow_test` (11 checks at ring 3, 6 at ring 0; Open Watcom, GCC and clang): a sparse stack's reserved bottom, growth by recursion and by a frame of four pages, `STACK_COMMITTED`, and `XCPT_UNABLE_TO_GROW_STACK` left by `longjmp`. Unit test: `a_sparse_stack_grows_down_and_runs_out_once`. Before, a thread that overflowed its stack wrote over the memory below it without a fault.
- **Phase 3:** `mem_protect_test` checks that the program's code can be read but not written (35 checks); unit test `read_only_objects_lose_only_their_own_pages`. The suite passes at both rings and with GCC-built samples, so none of them writes its own code.
- **Found on the way:**
  - Phase 3's first run left the program's code writable: its objects were protected before the process tables were first written, which then made every page writable again. `apply_page_changes` now writes the tables first.
  - A DLL object was given its virtual size (at least a page), while all its pages were loaded; an object smaller than its pages had file data written past its block. It now gets its pages' size (`load_dll_impl`).
  - A `DosAllocMem` that restricted pages a freed block had left in a TLB did not fault until `flush_tlbs` was also called there.
  - The `warpine-os2` crate's `set_mem` documentation gave wrong flag values; corrected.
