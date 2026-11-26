# ADR-0131: No-execute guest memory: PAE paging with EFER.NXE, strict with an opt-out

- **Status:** Accepted; supersedes the page-table format of ADR-0088, ADR-0114 and ADR-0115 (their mappings stay); extends ADR-0113
- **Date:** 2026-11-26
- **Sources:** `src/loader/paging.rs`, `src/loader/vmem.rs`, `src/loader/shared_arena.rs`, `src/loader/vcpu.rs`, `src/loader/seh.rs`, `src/loader/kvm_backend.rs`; `samples/nx_test`; Intel SDM vol. 3, 4.4 (PAE paging) and 4.6 (access rights, the XD bit); IBM *Control Program Programming Reference* (`DosAllocMem`, `DosSetMem`: `PAG_EXECUTE`); Open Watcom's `bsexcpt.h` (`XCPT_EXECUTE_ACCESS`)

## Context

Guests run with 32-bit paging (ADR-0088), which has no execute permission: every readable page can be executed. A program's stack, its heap and its data objects are therefore all executable, so a buffer overflow that writes code there can run it.

`DosAllocMem` and `DosSetMem` take `PAG_EXECUTE`, and LX objects carry an executable flag (`LX_OBJECT_EXEC`), but OS/2 on 32-bit x86 without PAE never enforced either. Some legacy programs may run code from memory allocated without `PAG_EXECUTE`.

The processor's execute-disable bit (XD, also called NX) exists only in PAE and long-mode paging, and only when `EFER.NXE` is set.

## Decision

- **PAE paging.** Every guest thread runs with `CR4.PAE` and `EFER.NXE`:
  - entries are 64 bits; a table has 512 of them; large pages are 2 MB;
  - CR3 holds a page-directory-pointer table (PDPT) of four entries;
  - the guest's KVM CPUID gets one leaf, `0x80000001` with EDX's NX bit (and `0x80000000` naming it), which KVM needs before it accepts `EFER.NXE`. No other leaf is set, so what programs see from `CPUID` is otherwise unchanged.
- **The same mappings in a new format.** Every address maps as before (ADR-0088, 0114, 0115), in 2 MB chunks:
  - **Per-thread block** (still four pages):
    - the page directory for the first gigabyte, which holds all guest memory;
    - the table for the 2 MB holding `TLM_PAGE`, the FS window and the ring-0 window;
    - the thread-local page;
    - the ring-0 page, whose TSS is followed by the PDPT at `PDPT_OFFSET`. CR3 points at that PDPT, and the block's base is found from it.
  - **Shared PDPT entries.** Entries 1–3 lead to three directories every thread shares, which map the rest of the 4 GB one-to-one with large pages, as today.
  - **Process tables** (program area 8, allocator 96, shared arena 16, then the three high directories) move into the 2 MB after the API thunks (`0x01200000`, `PROCESS_TABLES`). The thunks use 29 KB of their old 4 MB chunk, so that space was free. It becomes a supervisor-only large page. The paging arena then holds only blocks: 495 + 256 = 751, so the 736-thread limit of ADR-0115 stays.
- **Execute permission.** A page is executable only when:
  - it lies in a module's executable object (`PageMap::executable`, the objects marked by `protect_objects`, EXE and DLLs alike);
  - or it is a `DosAllocMem` / `DosSetMem` page whose state holds `PAG_EXECUTE`;
  - or it is a shared block held with `PAG_EXECUTE`;
  - or it is the API thunks (and the exit trap) or the IDT's ring-0 stubs.

  Everything else is no-execute: stacks, Warpine's own allocations, data objects, the TIB, PIB and environment, the system area and memory above guest RAM.
- **Faults.** An instruction fetch from a no-execute page is a page fault with the error code's instruction-fetch bit set. It is never retried as a stale TLB entry (unless the page has become executable). It raises `XCPT_ACCESS_VIOLATION` with `ExceptionInfo[0]` = `XCPT_EXECUTE_ACCESS` (Open Watcom's `bsexcpt.h`) and the address, so a handler sees it as any other access violation.
- **Strict, with an opt-out.**
  - NX is on for every 32-bit process.
  - `WARPINE_NX=off` turns it off for a run: every page is then executable, as before.
  - A per-program setting comes with the per-application manifest (TODOs, Security).
  - 16-bit (NE) processes run with every page executable: their code segments are not marked page by page, and their protection is the segment descriptors'.
- **Losing execute permission** counts as losing access (`loses_access`), so it flushes every vCPU's TLB as a lost write does.

## Consequences

- Code injected into a stack, a heap or a data object no longer runs: it traps as an access violation. This closes the most common exploit path left inside the guest.
- A legacy program that runs code from memory without `PAG_EXECUTE` traps too, until it is run with `WARPINE_NX=off`. The crash report names the fetch, so the cause is visible.
- Page tables take twice the memory (8-byte entries). The per-thread block keeps its four pages; the process tables grow from 60 to 123 pages in their own large page.
- The ring-0 page's stack loses the room the PDPT takes, 32 bytes and alignment, about 0.2 KB.
- Unverified (Warpine's own choices):
  - `XCPT_EXECUTE_ACCESS` in `ExceptionInfo[0]` for a fetch fault;
  - no-execute for stacks and for memory allocated without `PAG_EXECUTE` (OS/2 accepted both);
  - NX off for 16-bit processes.

## Addendum (2026-11-26): hosts that cannot run PAE guests

Some of GitHub's CI runners (an AMD EPYC 9V74 under Hyper-V, kernel 7.0.0-1017-azure, KVM nested) triple-fault every PAE guest at its first instruction, with or without `EFER.NXE` and the CPUID leaf; other runners, and the hosts this was developed on, run them. On GitHub CI only (`GITHUB_ACTIONS`, passed into the container by `ci/run.sh`), the integration tests first start `hello`; when it cannot start, every KVM test is skipped with a warning rather than all failing. Elsewhere such a failure still fails the tests. Execute protection is a requirement, not an option: Warpine does not fall back to 32-bit paging (without NX) on such hosts; a host that cannot run PAE guests cannot run Warpine.

Later runs showed the failure is per process, not per host: in one job 24 tests passed and 219 triple-faulted, interleaved, so the probe above cannot tell such hosts apart. The guest's memory is now mapped on a 2 MB boundary, so the host can back the guest's 2 MB pages with large pages of its own; whether that cures those hosts is being watched in CI (unverified).

The alignment did not help, and the failure is per host after all: such a runner fails every 32-bit guest (the same 24 of 243 integration tests pass each time, those running 16-bit programs or no guest code). The probe is gone. Instead `ci/matrix.sh`, on GitHub only, reports an integration-test run that failed with guest triple faults as a warning on the run's page rather than a failure; any other failure still fails the job, and unit tests run apart so they are never excused.

The PDPTs move out of the ring-0 page: each thread's PDPT now lies in a table after the process tables, in the same supervisor-only large page (`PDPTS`, one 32-byte slot per paging block), and CR3 names that slot. The ring-0 page holds the TSS and the ring-0 stack, which the processor writes while it delivers an exception; a nested host that write-protects pages holding page tables (as shadow paging does) would have to emulate those writes, the suspected cause of the triple faults at the first API call. CI also prints each runner's processor and KVM settings (`npt`, `ept`, `nested`), to tell such hosts apart. Whether this cures them is being watched in CI (unverified).

To check such fixes without the whole matrix, `.github/workflows/pae-probe.yml` (started by hand, `gh workflow run pae-probe.yml`) runs eight jobs at once in the pinned image, each printing its host and running only `hello` and `nx_test` (`ci/matrix.sh probe`), with no warning in place of a failure: a failing job shows a host that still cannot run PAE guests.
