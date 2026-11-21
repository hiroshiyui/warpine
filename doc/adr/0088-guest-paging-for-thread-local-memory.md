# ADR-0088: Guest paging for thread-local memory

- **Status:** Accepted; its addresses amended by ADR-0094 (`TLM_PAGE` moved into the system area, the page table covering that 4 MB) and ADR-0110 (a four-page block: the low 64 KB not present, the FS window to the thread's TIB); its security measures against a guest at ring 0 (item 5: the watch, `paging_violation`, the repair) superseded by ADR-0113, whose step 5 removed them once every guest ran at ring 3
- **Date:** 2026-11-20
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosAllocThreadLocalMemory` and `DosFreeThreadLocalMemory`. A thread-local memory area of 32 DWORDs; up to 8 DWORDs per call; one pointer that is the same for every thread, each thread seeing its own words through it.
  - Intel SDM vol. 3, chapter 4: 32-bit paging, 4 MB pages with `CR4.PSE`.
  - The Linux KVM API: guest page tables live in guest memory and are walked by the processor under EPT / NPT. The host side is unchanged.
  - The code: `src/loader/paging.rs` (new), `vcpu.rs` (`setup_vcpu_segments_32bit`, `run_vcpu`, `end_thread`), `doscalls.rs` (`DosCreateThread`), `guest_mem.rs`, `vm_backend.rs` / `kvm_backend.rs` (CR3), `constants.rs`; ADR-0086 (the deferral).
  - The sample: `samples/tlm_test`.

## Context

Warpine's guests run unpaged: CR0.PG is off, so a linear address is a guest-physical address, and every vCPU sees the same memory. The host reads guest memory the same way: `guest_mem.rs` takes a guest address as an offset into the one mapping.

`DosAllocThreadLocalMemory` gives one pointer that every thread uses, and each thread reads and writes its own words through it. In one flat, shared address space that cannot be done:
- threads run at the same time on separate vCPUs, so the area's contents cannot be swapped when a thread switches;
- the pointer goes through DS like any other data, so a segment base cannot single it out.

ADR-0086 left the two calls out for this reason.

## Options

1. **Paging per vCPU (recommended).**
   - Turn on 32-bit paging with `CR4.PSE`.
   - One page directory per thread:
     - 4 MB pages map all of guest memory one-to-one;
     - the 4 MB region holding the thread-local page goes through a 4 KB page table;
     - in that table, one entry maps the thread's own page, and the rest stay one-to-one.
   - About 8 KB of tables per thread, built by Warpine in memory the guest's allocator never hands out.
   - Each vCPU's CR3 points at its thread's directory.
   - The host translates the thread-local page:
     - API arguments that point into it go to the calling thread's own page;
     - `guest_mem` learns the current thread's page through a thread-local set when the vCPU starts;
     - other host threads (the GUI) see thread 1's.
2. **Leave the calls out.** Programs that use them fail at load or get `ERROR_INVALID_FUNCTION`. Few programs use them (the C runtimes keep their own per-thread data, reached through the TIB).
3. **A fake.** Give each call's caller a separate block. That breaks the one contract the calls have, the shared pointer, and would corrupt data quietly. Rejected.

## What paging changes beyond these two calls

- **Safety.** EPT / NPT still bound the guest to its own memory, so page tables the guest corrupts can only map memory it could already reach: a guest crash, not an escape. The tables sit outside every block the guest allocates, so only a stray write can reach them.
- **Faults.** A page fault (#PF) becomes possible. With everything mapped present and writable it should not happen; an unexpected one is reported like other unhandled exceptions.
- **Later uses.** Paging is what `PAG_GUARD` guard pages, stack growth, decommitted pages (`DosSetMem`) and read-only memory (`PAG_READ` without `PAG_WRITE`) need. Today all of them are accepted and not enforced. This ADR does not decide those; it makes them possible.
- **16-bit (NE) programs** run in protected mode without paging today. They would keep that: the 4 MB one-to-one map is transparent to them.
- **Cost.** Building the tables at thread creation is a few microseconds, and with 4 MB pages there are few TLB misses.

## Decision

Option 1, paging per vCPU.

1. **Tables.**
   - Each 32-bit thread has a paging block of three pages from the allocator (`paging_block`): a page directory, one page table and the thread's own thread-local page.
   - Directory entries 1–1023 are 4 MB pages mapping 4 MB–4 GB one-to-one; entry 0 leads to the table.
   - The table maps the first 4 MB one-to-one except `TLM_PAGE` (`0x9F000`, below 1 MB, between the environment page and 1 MB), which leads to the thread's own page.
   - Every entry is present, writable and user. So nothing faults that did not fault unpaged; memory past guest RAM still exits to Warpine as MMIO.
2. **vCPUs.**
   - `setup_vcpu_segments_32bit` takes the block's address: CR3 = the block, `CR4.PSE` and `CR0.PG` set (0 = unpaged).
   - Thread 1 gets its block when its vCPU is made. `DosCreateThread` makes one for each thread when the creating thread runs paged, and `end_thread` frees it with the stack and TIB.
   - A reused vCPU gets the new thread's CR3.
   - The 16-bit (NE) path stays unpaged.
3. **Host side.**
   - `run_vcpu` tells `guest_mem` which thread-local page belongs to its host thread (`set_thread_tlm_backing`, a thread-local). `GuestMemory::ptr_at`, the one door to guest memory, sends an access lying wholly inside `TLM_PAGE` there.
   - API calls see a thread's own words, and so do writes into them (`DosQuerySysInfo` into a thread-local word, for example).
4. **The calls.**
   - `DosAllocThreadLocalMemory` (454) and `DosFreeThreadLocalMemory` (455) keep a process-wide 32-bit map of the area's words (`TlmArea`, `SharedState::tlm`).
   - An allocation takes the first run of `cb` free words (1–8) and returns `TLM_PAGE + 4 × index`.
5. **Security** *(superseded by ADR-0113: guests run at ring 3, where they cannot touch their paging, and step 5 removed the watch, `paging_violation` and the repair; the MMIO storm limit and `finish_mmio` stay)*. The guest runs at ring 0, so it can rewrite its tables and load CR3 or CR0 itself. On OS/2 it could do neither, because the instructions are privileged and the tables are not the program's. EPT / NPT keep it inside its memory; Warpine adds:
   - **The host never follows the guest's tables.**
     - `guest_mem` translates only `TLM_PAGE`, to the page Warpine chose when the thread started.
     - A CR3 counts only when it is a whole, page-aligned block in the allocator's region (`valid_block`), and the arithmetic is checked, so no guest value can reach a host address outside guest memory.
   - **A guest that changes its paging ends as a trap (`TC_TRAP`, exit code 1).**
     - A watch thread kicks every vCPU every 250 ms (`start_paging_watch`).
     - Each kicked exit checks the thread's paging (`paging_violation`): paging off, a CR3 that is not its block, or tables that differ from Warpine's (ignoring the accessed and dirty bits, which Warpine sets from the start).
     - The kick also ends a vCPU that spins inside KVM after a wild CR3 and never exits on its own.
   - **The exit list still runs.** Before thread 1 runs its routines after any fault, `end_by_fault` rewrites the thread's directory and table (`repair_paging_block`; the thread-local words stay), drops the exception KVM was still delivering (`clear_pending_events`), and restores the starting segment state and CR3.
   - **Endless accesses past guest RAM end too.** Such accesses read as zero, but 1,000 in a row with no other exit except kicks (`MMIO_STORM_LIMIT`, about a second) end the thread as a trap. They happen when the guest points a table entry, or a loop, past its memory.
   - **No redirection is lost.** KVM finishes an MMIO exit on the next `KVM_RUN`, and changing the registers before that loses the change. `finish_mmio` runs once with `immediate_exit` set after every MMIO exit, so a termination's exit list or a trap can take over at once.
   - **Crash reports** show CR3 and the paging state: off, intact, damaged by the guest, or a CR3 the guest loaded (`PagingStatus`).

## Warpine's choices (unverified)

- The area's address (`0x9F000`) and its 32 DWORDs at the start of the page.
- New threads' words starting zero.
- The first fitting run handed out; `ERROR_NOT_ENOUGH_MEMORY` when no run of `cb` words is free.
- `ERROR_INVALID_PARAMETER` for:
  - `cb` of 0 or above 8, or a NULL `pp`;
  - freeing anything but a block's first word.
- Warpine's own host threads (the GUI's) seeing thread 1's page; a host access crossing the page's end not translated.
- A guest changing its own paging ending as a trap (`TC_TRAP`) rather than a general-protection exception it could handle; the 250 ms watch interval; 1,000 accesses in a row past guest RAM ending a thread as a trap.

## Consequences

- **Programs** using thread-local memory run, and each thread sees its own words.
- **Every 32-bit thread** runs with paging on. The mapping is one-to-one apart from one page, so programs see no difference, and EPT / NPT still bound the guest.
- **Cost:** 12 KB of guest memory per thread.
- **Later work:** guard pages, stack growth, decommitted pages and read-only memory now have a mechanism. Each needs its own decision (TODOs, "Memory protection on the page tables").
- **Coverage:** two more DOSCALLS entry points, 879 in all.
- **Tests:**
  - unit tests:
    - `paging.rs`: the tables, the word map, the calls' arguments, host accesses following the thread's page;
    - security properties: no entry maps a page elsewhere except the thread-local page; only valid blocks are accepted, whatever CR3 holds; translated host accesses stay inside guest memory for any offset and length; damaged tables are seen and repaired with the words kept; a guest changing its paging is caught;
    - `vcpu.rs`: a thread runs paged on its own block, or unpaged;
    - `session_threads.rs`: a failed `DosCreateThread` frees the page tables;
    - `crash_dump.rs`: the report shows the paging state;
  - `tests/integration.rs` `test_thread_local_memory`: `tlm_test`, 16 checks, two threads with their own words behind one pointer, one of them written by an API call;
  - `test_a_guest_attacking_its_paging_ends_as_a_trap`: `paging_abuse_test`, a hostile guest built five times:
    - it zeroes its directory, loads a CR3 past its memory, remaps its thread-local page, turns paging off, or reads past its memory in a loop;
    - each ends with exit code 1, its exit list run once with `TC_TRAP`, and one crash dump naming what it did; none hangs.
  - `test_sigterm_during_accesses_past_guest_memory_runs_the_exit_list`: the MMIO-completion fix. It fails without `finish_mmio`.
  - every other sample runs paged.
