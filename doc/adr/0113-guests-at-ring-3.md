# ADR-0113: Running 32-bit guests at ring 3

- **Status:** Proposed (for review before any code)
- **Date:** 2026-11-21
- **Sources:**
  - Intel SDM vol. 3: privilege levels and checks (5.5–5.8), stack switching through the TSS on an interrupt from ring 3 (6.12.1, 7.2), interrupt and trap gates and their DPL, `SYSENTER` / software interrupts, CR0.WP and the U/S page bit (4.6).
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

## Decision (proposed)

**Run 32-bit guests at CPL 3; keep ring 0 for nothing the guest executes.**

1. **Segments:**
   - new descriptors: 32-bit code and data at DPL 3, an FS descriptor (the FS window, ADR-0110) at DPL 3, and the 16-bit data and code tiles at DPL 3;
   - the 16-bit tiles were DPL 2 so that RPL-2 selectors (OS/2's IOPL convention) load. At CPL 3, RPL 2 selectors to DPL 3 descriptors still load (max(CPL, RPL) ≤ DPL);
   - selector values stay as programs know them: OS/2 programs save and reload them (Open Watcom's `setjmp`), so only the RPL bits change (0x0B, 0x13, 0x1B, …), and Warpine accepts both forms where it compares selectors.
2. **A TSS per vCPU,** with SS0:ESP0 pointing at a small ring-0 stack in the system area (one page per thread, mapped supervisor-only). It is needed because an exception from ring 3 switches to the ring-0 stack. TR is loaded through `KVM_SET_SREGS`.
3. **The IDT:**
   - gates stay DPL 0, so a guest's `INT n` faults (#GP) instead of reaching a handler;
   - the stubs run at ring 0 on the TSS stack. They still `push vector; int3`, and the host reads the frame there, with SS:ESP of the guest pushed by the CPU, instead of computing the pre-fault ESP (`frame_base + 20` today).
4. **Paging:**
   - the paging block, the GDT, IDT, TSS and ring-0 stacks are mapped supervisor-only (U/S = 0);
   - CR0.WP is set, so ring 0 cannot write read-only pages either;
   - the guest can no longer load CR3 or CR0, or write the tables; `os2-paging-watch` and `paging_violation` stay as a second check, then may go.
5. **API thunks and callbacks are unchanged in kind:**
   - INT3 in the stub area is a debug exit for KVM (`USE_SW_BP`) at any CPL, so the calls, `CALLBACK_RET_TRAP`, `XCPT_RET_TRAP` and `EXIT_TRAP_ADDR` work as now;
   - the stub pages are user-readable and executable, not writable.
6. **What changes with them:**
   - **Far16** (vcpu.rs: `JMP FAR` / `LSS` with tiled selectors): the #GP that LSS raises today because a DPL-2 tile cannot be SS at CPL 0 goes away. The emulation's selector checks and the CS / SS it restores take RPL 3.
   - **The NE path** runs unpaged at CPL 0 today. It moves to CPL 3 with the same descriptors, or stays as it is in a first step: 16-bit programs are a separate process.
   - **Exception dispatch and `XCPT_CONTINUE_EXECUTION`** restore EFLAGS from the context record: IOPL and IF must be masked so a guest cannot raise its IOPL through a context record. CS and SS from a context record are not applied today and must not be.
   - **Signals and `DosUnwindException`** build frames on the guest's stack as now (ring 3).
   - **The GDB stub** reports CPL 3 selectors; its RIP-range checks are unchanged.
   - **Crash reports** show the ring-0 frame and the guest's SS:ESP from it.
7. **Steps:**
   1. TSS and ring-0 stacks with guests still at ring 0 (no behaviour change);
   2. DPL-3 descriptors and CPL 3 for LX threads, behind `WARPINE_RING=3`, with the whole suite and the GCC / clang builds passing;
   3. the default switched, U/S and CR0.WP set, `WARPINE_RING=0` kept for a release as a way back;
   4. the NE path;
   5. the paging watch removed once nothing needs it.

## Open questions

- **Selectors in saved state:** programs that compare a selector they saved with a constant (0x10, 0x18) would see 0x13, 0x1B. Open Watcom's runtime only saves and reloads them, but other compilers' runtimes need a look.
- **IOPL code:** OS/2 lets IOPL segments run at ring 2 with I/O privilege; Warpine has no `DosPortAccess` and no I/O, so ring 2 is not needed now.
- **Performance:** a privilege change on every exception and the stack switch cost little next to the VM exit each API call already is; to be measured.
- **The same design for XE** (ADR-0109): ring 3 keeps the psABI's red zone usable.

## Consequences (expected)

- The processor, not a 250 ms poll, keeps guests out of their page tables and control registers, and privileged instructions fault at once as `XCPT_PRIVILEGED_INSTRUCTION`, as on OS/2.
- More of Warpine's guest-facing state is enforced by hardware, at the cost of a TSS per thread and changes in the fault, Far16 and NE paths, each covered by the existing samples and tests.
