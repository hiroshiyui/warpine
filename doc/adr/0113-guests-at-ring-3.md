# ADR-0113: Running 32-bit guests at ring 3

- **Status:** Accepted, all five steps done: every program runs at ring 3, with no way back (step 5, after v0.0.10)
- **Date:** 2026-11-21
- **Sources:**
  - Intel SDM vol. 3: privilege levels and checks (5.5–5.8), stack switching through the TSS on an interrupt from ring 3 (6.12.1, 7.2), the gate DPL check that `INT n`, `INT3` and `INTO` make (6.12.1.1), the 32-bit TSS and its I/O map base (8.2.1), CR0.WP and the U/S page bit (4.6).
  - The KVM API: `KVM_SET_GUEST_DEBUG` (`KVM_GUESTDBG_USE_SW_BP`), `KVM_SET_SREGS` (TR, segment DPLs).
  - IBM's *Control Program Programming Reference* and the OS/2 2.0 architecture: OS/2 applications run at ring 3, with IOPL code at ring 2 and the kernel at ring 0.
  - A survey of Warpine's own code (vcpu.rs, descriptors.rs, paging.rs, ne_exec.rs, seh.rs, signals.rs, gdb_stub.rs) for what assumes ring 0.

## Context

32-bit guests run at CPL 0:
- Every segment is DPL 0 (the 16-bit data tiles DPL 2).
- Every page is user, writable and present (ADR-0088).
- There is no TSS.

A guest can therefore load CR3, change CR0, write its own page tables, or execute any privileged instruction. Warpine only notices afterwards:
- the `os2-paging-watch` thread kicks every paged vCPU every 250 ms, and `paging_violation` compares the tables and control registers with what Warpine set up;
- a difference ends the program as a trap (ADR-0088).

EPT / NPT still bound the guest to its own memory, so this is not a hypervisor escape. But it is a window in which a guest controls its own translation, and a rule enforced by polling rather than by the processor.

OS/2 itself runs applications at ring 3. ADR-0109's red-zone question for XE points the same way: at ring 3 an interrupt switches stacks and cannot overwrite a function's red zone.

## Decision

**Run 32-bit guests at CPL 3; keep ring 0 for nothing the guest executes.**

1. **Segments:**
   - the descriptors programs use become DPL 3: 32-bit code (0x08) and data (0x10), FS (0x18, the FS window, ADR-0110), and the 16-bit data and code tiles;
   - the 16-bit tiles were DPL 2 so that RPL-2 selectors (OS/2's IOPL convention) load. At CPL 3, RPL 2 selectors to DPL 3 descriptors still load (max(CPL, RPL) ≤ DPL);
   - selector values stay as programs know them: OS/2 programs save and reload them (Open Watcom's `setjmp`), so only the RPL bits change (0x0B, 0x13, 0x1B, …), and Warpine accepts both forms where it compares selectors;
   - ring 0 gets descriptors of its own after the code tiles: code (`KERNEL_CODE_SEL`), data (`KERNEL_DATA_SEL`) and the TSS (`TSS_SEL`).
2. **A TSS per thread,** in a fifth page of the thread's paging block (ADR-0088), its *ring-0 page*. Each thread's tables map it at one linear address, the *ring-0 window* (`RING0_WINDOW`, the page after the FS window), so one TSS descriptor serves every thread.
   - The page starts with the TSS: SS0 is ring-0 data, ESP0 the top of the page, so the rest of the page is the ring-0 stack an exception from ring 3 switches to.
   - The I/O map base is the TSS's size: no I/O permission bitmap, so ring 3 may use no port.
   - TR is loaded through `KVM_SET_SREGS` as a busy 32-bit TSS, never with `LTR`. An unpaged thread (16-bit programs, or a 32-bit thread whose paging block could not be allocated) has no ring-0 page and keeps TR's reset state; it runs at ring 0.
3. **The IDT:**
   - gates stay DPL 0, so a guest's `INT n` faults (#GP) instead of reaching a handler — **except the breakpoint gate (vector 3), which is DPL 3.** The processor checks a gate's DPL for `INT3` as for `INT n` (SDM 6.12.1.1): at DPL 0, every API thunk's `INT3` would raise #GP from ring 3 instead of the #BP that KVM turns into a debug exit. This opens nothing: Warpine takes an `INT3` as an API call only at a thunk address;
   - the stubs run at ring 0 on the TSS stack. They still `push vector; int3`, and the host reads the frame there, with SS:ESP of the guest pushed by the CPU, instead of computing the pre-fault ESP (`frame_base + 20` today).
4. **Paging:**
   - the paging block, the GDT, IDT and the ring-0 window are mapped supervisor-only (U/S = 0);
   - CR0.WP is set, so ring 0 cannot write read-only pages either;
   - the guest can no longer load CR3 or CR0, or write the tables; `os2-paging-watch` and `paging_violation` stay as a second check, then may go.
5. **API thunks and callbacks are unchanged in kind:**
   - INT3 in the stub area is a debug exit for KVM (`USE_SW_BP`) at any CPL, given the DPL-3 breakpoint gate (item 3), so the calls, `CALLBACK_RET_TRAP`, `XCPT_RET_TRAP` and `EXIT_TRAP_ADDR` work as now;
   - the stub pages are user-readable and executable, not writable.
6. **What changes with them:**
   - **Far16** (vcpu.rs: `JMP FAR` / `LSS` with tiled selectors): the #GP that LSS raises today because a DPL-2 tile cannot be SS at CPL 0 goes away. The emulation's selector checks and the CS / SS it restores take RPL 3.
   - **The NE path** runs unpaged at CPL 0 today. It moves to CPL 3 with the same descriptors, or stays as it is in a first step: 16-bit programs are a separate process.
   - **Exception dispatch and `XCPT_CONTINUE_EXECUTION`** restore EFLAGS from the context record: IOPL and IF must be masked so a guest cannot raise its IOPL through a context record. CS and SS from a context record are not applied today and must not be.
   - **Signals and `DosUnwindException`** build frames on the guest's stack as now (ring 3).
   - **The GDB stub** reports CPL 3 selectors; its RIP-range checks are unchanged.
   - **Crash reports** show the ring-0 frame and the guest's SS:ESP from it.
7. **Steps:**
   1. **Done:** the TSS and ring-0 page per thread, the ring-0 descriptors and the DPL-3 breakpoint gate, with guests still at ring 0 (no behaviour change);
   2. **Done:** DPL-3 descriptors and CPL 3 for LX threads, behind `WARPINE_RING=3`, with the whole suite and the GCC / clang builds passing (see *Step 2* below);
   3. **Done:** the default switched, U/S and CR0.WP set; `WARPINE_RING=0` kept for one release as a way back (from v0.0.9 to v0.0.10), then removed (see *Step 3* below);
   4. **Done:** the NE path (see *Step 4* below);
   5. **Done:** the paging watch kept as a second check for one release after the switch, then removed (see *Step 5* below).

## Step 2: how ring 3 works (`src/loader/ring.rs`)

- **The ring is per process.** `setup_guest` (LX only) reads `WARPINE_RING` before writing the GDT (`set_guest_ring`); 16-bit programs never set it. A paged thread starts at that ring; an unpaged one (no paging block) stays at ring 0, with a warning.
- **Descriptors** (`setup_idt`): at ring 3 the descriptors programs use (0x08, 0x10, 0x18, the 16-bit aliases 0x20 / 0x28, every data and code tile) are DPL 3, and every gate's code selector is `KERNEL_CODE_SEL`. At ring 0 they are as before.
- **An exception's frame.** From ring 3 the processor switches to the TSS's stack and pushes EIP, CS, EFLAGS, ESP and SS there; the stub adds its two words. At the stub's `INT3` exit, `lower_ring3_frame` reads that frame from the thread's ring-0 page (found from its thread-local page, not by walking tables), writes the first five words below the guest's ESP, and puts the vCPU back at ring 3 on that stack: CS and SS from their GDT descriptors (`gdt_segment`), so a 16-bit stack's SS and SP work too. Everything after it reads the state an exception at ring 0 left, unchanged. A guest stack that cannot take the frame ends the process as a trap, as a triple fault did at ring 0.
- **Privileged instructions:** a #GP with error code 0 from ring 3 at an instruction only ring 0 may execute (`is_privileged_instruction`: `HLT`, `CLI`, `STI`, port I/O, moves to control and debug registers, `CLTS`, `INVD`, `WBINVD`, `WRMSR`, `RDMSR`, `LGDT`, `LIDT`, `LMSW`, `INVLPG`, `LLDT`, `LTR`) is `XCPT_PRIVILEGED_INSTRUCTION`, continuable.
- **EFLAGS from a guest** (a context record on `XCPT_CONTINUE_EXECUTION`, an unwind's resume, the Far16 bypass) keep only CF, PF, AF, ZF, SF, TF, DF, OF, AC and ID (`guest_eflags`), at either ring.
- **Selectors Warpine writes:** `DosRaiseException`'s context record and the Far16 bypass use the thread's ring's flat selectors (`FaultContext::flat`, `flat_selector`); the Far16 check compares selectors without their RPL.
- **A kick inside a stub:** a thread kicked between the processor's delivery and the stub's `INT3` is at ring 0; it takes no signal or termination there (`signal_action`), since the stub exits at once.
- **Tests that pin a ring:** the paging-abuse and HLT tests test ring 0's defences and run with `WARPINE_RING=0`; their ring-3 counterparts (`at_ring_3_a_guest_cannot_reach_its_paging`, `at_ring_3_hlt_ends_the_process_by_an_exception`) check that the first privileged instruction ends the program by an unhandled exception, its tables intact (the abuse sample reads CR3 before it registers its exit list; HLT's exit list gets `TC_EXCEPTION`). `ring_test` runs at both rings.

## Step 3: ring 3 by default, Warpine's pages out of reach (`paging.rs`)

- **The default is ring 3.** `WARPINE_RING=0` (only that value) runs 32-bit guests at ring 0, with a warning, for one more release; any other value means ring 3.
- **The paging arena.** Paging blocks no longer come from the allocator, whose 4 MB pages every thread maps user-writable: a thread could have rewritten another thread's tables. They come from the *paging arena*, from the page after the ring-0 window (`PAGING_ARENA_BASE`, 0x01842000) to the end of the 4 MB the TLM table maps (0x01C00000): 191 blocks (`PAGING_BLOCKS`, `PagingArena`). The TLM table maps that 4 MB page by page, so its pages can be supervisor-only. (ADR-0115 extends the arena to the allocator, 482 blocks.)
- **Supervisor-only pages** (`supervisor_only`), in every thread's TLM table: the GDT, the IDT and its stubs (the system area below the TIB), the ring-0 window and the arena. The TIB, PIB, environment, thread-local page, information segments and FS window stay user pages. The processor's own accesses to the descriptor tables and the TSS, and the stubs it runs at ring 0, are supervisor accesses.
- **The API thunks' 4 MB are read-only** (their directory entry, `STUB_CHUNK`), and **CR0.WP** is set for every paged thread, so a guest at ring 0 cannot rewrite a thunk either.
- **What a ring-3 guest can no longer do:** read or write its own or another thread's page tables, TSS or ring-0 stack, or the GDT and IDT; write a thunk; load CR3 or CR0. The paging watch stays until step 5, for `WARPINE_RING=0`.

## Step 4: 16-bit programs at ring 3 (`ne_exec.rs`)

- **At ring 3 a 16-bit program runs paged,** with a paging block like a 32-bit thread's: its TSS in the ring-0 window, Warpine's pages supervisor-only, the thunks read-only, CR0.WP. Without paging, descriptors alone would protect little: an unpaged guest at ring 3 could still rewrite the GDT in memory and give itself a gate to ring 0.
- **Descriptors:** `setup_guest_ne` reads `WARPINE_RING` before `setup_idt`, so the tiles are DPL 3; the descriptors it rewrites for the program's own segments (their exact limits) are DPL 3 too. The entry CS, DS, SS and the flat FS / GS carry RPL 3; a far call's return CS, pushed by the processor, keeps it.
- **Exceptions:** an exception from 16-bit ring-3 code goes through the same 32-bit gates; `lower_ring3_frame` moves its frame to the 16-bit stack (SS.base + SP). With no 16-bit exception handling, an unhandled one ends the program with a crash report, as before.
- **`WARPINE_RING=0`** keeps the 16-bit path exactly as it was: unpaged, at ring 0.
- **The low 64 KB** are not present for a 16-bit program at ring 3, as for 32-bit ones (ADR-0110); Warpine's BIOS data area copy there is out of its reach.

## Step 5: ring 3 only

- **`WARPINE_RING` is gone,** with ring 0 as a mode: the ring is a constant (`USER_RING`, `user_selector`), so the per-process ring (`guest_ring`, `set_guest_ring`, `thread_ring`) and the ring-0 branches of the descriptor setup, the segment setup and the NE path are gone too.
- **Every thread runs paged.** A thread that cannot get its page tables does not start (`DosCreateThread` returns `ERROR_NOT_ENOUGH_MEMORY`; thread 1, of a 32-bit or 16-bit program, ends the process with an error) instead of running unpaged at ring 0, which DPL-3 descriptors no longer allow.
- **The ring-0 defences are gone** (ADR-0088 item 5): the `os2-paging-watch` thread that kicked every vCPU every 250 ms, `paging_violation`, and the integrity check and repair of a thread's tables (`paging_block_intact`, `repair_paging_block`). Crash reports still say whether paging is on and whether CR3 is the thread's own block (`PagingStatus::Own`, `ForeignCr3`).
- **Warpine does not write its protected memory for a guest.** Removing the watch showed what it still covered: Warpine writes many results through pointers a guest gives (`DosQueryMem`'s `*pcb`, for one), bypassing the page tables, so a guest could have pointed one into its own page directory, in the supervisor-only arena, and had Warpine corrupt it, the first step to reaching supervisor pages (EPT still bounded it to its own memory). The ordinary write paths of guest memory (`write`, `write_bytes`, `slice_mut`) now refuse any write that touches the GDT, IDT and stubs or the paging arena (`guest_mem::protected`), checked on the address as given, before the thread-local page is translated; Warpine's own setup writes those structures through separate paths (`write_system`, `system_write`). An API call given such a pointer fails as with any address it cannot write.
- **Tests run at the one ring:** the paging-abuse variants each fault at their first privileged instruction (`a_guest_cannot_reach_its_paging`); `pgabuse_past` no longer reads CR3 and still tests the storm of accesses past guest RAM (`a_guest_reading_past_its_memory_ends_as_a_trap`, and SIGTERM during it); HLT ends a program with `TC_EXCEPTION`.

## Open questions

- **Selectors in saved state:** answered at review. No OS/2 program can know Warpine's 0x08 / 0x10 / 0x18, since OS/2's own flat selectors have other values; a program that compares a selector with a constant fails under Warpine at either ring. Using OS/2's values would be a decision of its own, needing a public source for them.
- **IOPL code:** OS/2 lets IOPL segments run at ring 2 with I/O privilege; Warpine has no `DosPortAccess` and no I/O, so ring 2 is not needed now.
- **Performance:** a privilege change on every exception and the stack switch cost little next to the VM exit each API call already is; to be measured.
- **The same design for XE** (ADR-0109): ring 3 keeps the psABI's red zone usable.

## Warpine's choices (unverified)

- **One page per thread** for the TSS and ring-0 stack together: about 3.9 KB of stack for an exception frame and the stub's two pushes, far more than they take.
- **The ring-0 window right after the FS window,** in the 4 MB the TLM table already maps, so the block needs no further table.
- **The descriptors programs use keep their selectors,** and ring 0 gets new ones at the end of the GDT, rather than moving the tiles.
- **The frame moved below the guest's ESP,** where the processor put it at ring 0, rather than teaching every path to read the ring-0 stack.
- **The list of privileged instructions** and the EFLAGS a program may set, from the SDM; OS/2's own lists were not available.
- **An unhandled privileged instruction ends with `TC_EXCEPTION`,** like every other unhandled exception in Warpine; at ring 0 `HLT` ended as a trap (`TC_TRAP`).
- **EFLAGS are masked at both rings,** not only at ring 3: no program sets IOPL or IF legitimately.
- **191 paging blocks:** a process can have at most 191 32-bit threads at once, whatever `THREADS=` says (the default is 64); past that, `DosCreateThread` returns `ERROR_NOT_ENOUGH_MEMORY`, as it did when no block could be allocated. More would need a second arena with its own page tables.
- **16-bit programs at ring 3 lose the low 64 KB,** where Warpine writes a copy of the BIOS data area; real OS/2 1.x programs reach the BIOS data through a selector OS/2 gives them, which Warpine does not provide either way.
- **The whole 4 MB of the thunks read-only,** with one directory entry, rather than only the thunks' pages; nothing else lies in it.
- **The system area below the TIB supervisor-only** as a whole (16 pages), not only the GDT and IDT pages.

## Consequences

- The processor, not a 250 ms poll, keeps guests out of their page tables and control registers, and privileged instructions fault at once as `XCPT_PRIVILEGED_INSTRUCTION`, as on OS/2.
- More of Warpine's guest-facing state is enforced by hardware, at the cost of a TSS per thread and changes in the fault, Far16 and NE paths, each covered by the existing samples and tests.
- **Step 1:** each paging block grows from four pages to five. The sample `ring_test` checks, on two threads, that the task register names a GDT TSS and that the segments are the flat ones at the current ring's RPL; run with the TSS missing, its TSS checks fail. Unit tests: the ring-0 page's TSS (`paging.rs`), TR for paged and unpaged threads, the ring-0 descriptors and the gates' DPLs (`vcpu.rs`).
- **Step 2:** with `WARPINE_RING=3` the whole suite passes (the ring-0 abuse tests pinned to ring 0, their ring-3 counterparts added), and `ring_test` checks `CLI` raising `XCPT_PRIVILEGED_INSTRUCTION` and a context record unable to raise IOPL. Unit tests: `ring.rs` (the variable, EFLAGS, the instruction list, descriptor decoding, moving a frame), and the ring-3 descriptors and segments (`vcpu.rs`).
- **Step 3:** ring 3 is the default. `ring_test` checks, built by Open Watcom, GCC and clang, that at ring 3 the GDT, the ring-0 window and the paging arena raise `XCPT_ACCESS_VIOLATION`, and at either ring that the thunks' 4 MB cannot be written (17 checks at ring 3, 11 at ring 0). Unit tests: the arena, the supervisor-only pages and the read-only thunks' entry, CR0.WP. The suite passes at the default ring and with `WARPINE_RING=0`.
- **Step 4:** `ne_ring_test` (assembly, Open Watcom) runs at ring 3 with its data and stack at its code's RPL, and `CLI` ends it as an unhandled #GP with its tables intact; with `WARPINE_RING=0` it runs at ring 0, where `CLI` goes through. `ne_hello` passes at both rings.
- **Step 5:** the suite passes with no ring-0 runs left; unit tests `protected_memory_takes_only_system_writes` (the GDT, IDT and arena refuse ordinary writes, take system ones; `TLM_PAGE` stays writable) and `the_paging_state_names_the_threads_own_block`.
- **Found, then fixed:** an `INT3` a program executed itself, outside the API thunks, ended the process as an unexpected breakpoint, with exit code 0 and no exit list. It now raises `XCPT_BREAKPOINT` to the program's handlers, as on OS/2 (`samples/xcpt_breakpoint_test`); the DPL-3 breakpoint gate makes it reach Warpine at ring 3 too.
