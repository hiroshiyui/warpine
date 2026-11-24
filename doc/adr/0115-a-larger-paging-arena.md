# ADR-0115: A larger paging arena: 482 threads per process

- **Status:** Accepted
- **Date:** 2026-11-22
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosCreateThread` and its return codes (`ERROR_NOT_ENOUGH_MEMORY`, `ERROR_MAX_THRDS_REACHED`).
  - Intel SDM vol. 3: 32-bit paging, 4 MB pages and the user/supervisor bit (4.3, 4.6).
  - The KVM API: `KVM_CHECK_EXTENSION` with `KVM_CAP_MAX_VCPUS` (4096 on the development machine).

## Context

Every thread runs paged at ring 3 on its own paging block of four pages: a page directory, the table for the 4 MB holding `TLM_PAGE`, the thread-local page and the ring-0 page (TSS and ring-0 stack) (ADR-0088, ADR-0113, ADR-0114). The blocks lie in the *paging arena*, which a guest must not reach. That arena was the rest of the TLM table's 4 MB, after the system area and the windows (`0x01842000`–`0x01C00000`). The TLM table maps that range page by page, so its pages could be made supervisor-only one by one. After the process tables (52 pages), it held 226 blocks.

So a process could have at most 226 threads, whatever `THREADS=` allows (up to 4095 for the session). `DosCreateThread` then failed with `ERROR_NOT_ENOUGH_MEMORY`, which a program reads as a shortage of memory, not of threads. KVM does not limit sooner: it allows 4096 vCPUs a VM, and Warpine gives each thread its own vCPU.

The 4 MB after the TLM table's (`0x01C00000`–`0x02000000`, up to the allocator, `DYNAMIC_ALLOC_BASE`) held nothing. Every thread's directory mapped it as a user-writable 4 MB page.

## Decision

1. **The arena runs up to the allocator.** `PAGING_ARENA_END` is `DYNAMIC_ALLOC_BASE`. The 4 MB after the TLM table's (`ARENA_CHUNK`) is mapped in every directory as one supervisor-only 4 MB page (`PTE_SUPERVISOR | PDE_LARGE`). The processor's own accesses (page walks, the TSS) are supervisor accesses. A guest at ring 3 reaches nothing there.
   - The arena stays one range: `supervisor_only`, `valid_block`, `guest_mem::protected` and `PagingArena` keep their single bounds.
   - It now holds 482 blocks (`PAGING_BLOCKS`): a process can have 482 threads, thread 1 and 481 more.
2. **A full arena is the process's thread limit.** `DosCreateThread` returns `ERROR_MAX_THRDS_REACHED` (164) when no block is free, as it does at the session's `THREADS=` limit. It gives the new thread's stack and TIB back and takes no session slot.

## Alternatives considered

- **Also use the 4 MB after the thunks** (`0x01400000`–`0x01800000`, free as well). That would make 738 threads, but the arena would become two ranges, and every check of it would need both. It stayed free for later; taken since (see *Later: the 4 MB after the thunks*).
- **Blocks from the allocator**, made supervisor-only through the process tables, with no fixed cap. The allocator's pages follow `MemoryManager` (`sync_allocations` makes allocated pages user-writable). Blocks would need a page state of their own, and `guest_mem::protected` would turn from two constant ranges into a lookup. That is more moving parts in the part of Warpine that keeps a guest out of its tables. Each thread also costs its stack and a host thread, so a few hundred threads is a reasonable bound for now.
- **Smaller blocks.** Every part is per thread: the directory leads to the thread's own TLM table, which holds its thread-local page, FS window and ring-0 window. The thread-local page is user-writable and the ring-0 page is not, so they cannot share a page.

## Consequences

- A process can have 482 threads (was 226); the session's `THREADS=` limit still applies across processes.
- A program that runs out of threads sees `ERROR_MAX_THRDS_REACHED`, whichever limit it hit.
- The 4 MB at `0x01C00000` are no longer user-writable. Nothing was placed there, and no program could rely on them.
- Tests: `the_tables_map_everything_one_to_one_but_the_thread_local_page` and `no_entry_maps_a_page_elsewhere_than_itself_but_the_thread_local_page` check the new directory entry; `dos_create_thread_fails_when_the_paging_arena_is_full`; `samples/threads_limit_test` in its `cap` mode (`a_process_gets_482_threads`: 481 threads beside thread 1, twice, `ERROR_MAX_THRDS_REACHED` each time).

### Later: the 4 MB after the thunks

- The 4 MB between the thunks' and the system area's (`EXTRA_CHUNK`, `PAGING_EXTRA_BASE` – `PAGING_EXTRA_END`, `0x01400000`–`0x01800000`) hold 256 more blocks: **738 threads a process**, thread 1 and 737 more.
- They are mapped in every directory as a supervisor-only 4 MB page, like `ARENA_CHUNK`; `supervisor_only` and `guest_mem::protected` cover them.
- The arena is two ranges now: `PagingArena` hands out slots, the first 482 after the process tables, the rest in the extra 4 MB (`block_base`, `block_slot`); `valid_block` accepts a block of either.
- Nothing had been placed there, and those 4 MB were user-writable before; no program could rely on them.
- Tests: the directory-entry tests and `only_the_arenas_blocks_are_accepted` (every slot's block and back), `protected_memory_takes_only_system_writes`, and `a_process_gets_738_threads` (`threads_limit_test cap`).

### Unverified

- OS/2's own limit of threads per process: Warpine's 738 (482 before) comes from its own layout, not from OS/2.
- `ERROR_MAX_THRDS_REACHED` for a process's own limit (the reference lists it for "too many threads", without saying which limit).

## Addendum (2026-11-24): 736 threads

The shared arena's eight page tables (ADR-0049 addendum) follow the allocator's in the process tables, before the paging blocks: 480 blocks after them, 736 in all (`PAGING_BLOCKS`), so a process can have 736 threads (`a_process_gets_736_threads`).
