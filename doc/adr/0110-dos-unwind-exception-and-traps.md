# ADR-0110: A real DosUnwindException, NULL pointers that trap, FS through a window, and C++ exceptions from OS/2 traps

- **Status:** Accepted; amends ADR-0088 (each thread's paging block), ADR-0103 (the SDK's unwinder) and ADR-0096 (the SDK's compiler flags)
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosUnwindException`, `DosRaiseException`, the exception handler's arguments, `EXCEPTIONREPORTRECORD` (`fHandlerFlags`, `ExceptionAddress`, `ExceptionInfo`), `XCPT_ACCESS_VIOLATION`'s information words.
  - Open Watcom's `<bsexcpt.h>`: `EH_UNWINDING`, `UNWIND_ALL`, `END_OF_CHAIN`, `XCPT_UNWIND`, `XCPT_*_ACCESS`, `_ERR` / `ERR`.
  - The test oracle, Open Watcom's runtime (its public source, Open Watcom Public License; not IBM's code):
    - `longjmp` (`clib/startup/a/stjmp386.asm`) calls `DosUnwindException(record at setjmp, &label, NULL)` with `label` right after the call, then reloads FS and GS with the selectors `setjmp` saved;
    - the C++ runtime (`cpplib/runtime/cpp/stabthrw.cpp`, `fsregist.cpp`) registers a handler per C++ frame, raises a throw with `DosRaiseException` (whose `ExceptionAddress` it leaves 0 "for the system to fill in"), and unwinds with `DosUnwindException(catching frame, excrec.addr, &excrec)`; its handler runs the frame's destructors when called with `EH_UNWINDING`.
  - Intel SDM vol. 3: page-fault error codes, descriptor loads.

## Context

`DosUnwindException` only cut the thread's handler chain: it called no handler and ignored `pTargetIP`. Open Watcom C++ programs therefore skipped every destructor between a `throw` and its `catch`: `wcxx_throw_test` printed `O` where `321O` was due.

Writing the oracle sample (`xcpt_unwind_test`) found two more faults:
- **NULL pointers did not trap.** 32-bit threads map the whole space one-to-one, page 0 included. A NULL write went to linear address 0 silently; on OS/2 it raises `XCPT_ACCESS_VIOLATION`. `rexx_api_test` had a `strlen(NULL)` that only this hid.
- **Reloading FS lost the TIB.** Selector `0x18` was a flat descriptor with base 0, and the TIB base was set only in each vCPU's hidden FS base. Open Watcom's `longjmp` reloads FS, after which `fs:[0]` (the chain head) was linear 0 and `DosGetInfoBlocks` returned a TIB at 0.

The SDK's C++ runtime also took the records of frames it unwound off the chain with `DosUnsetExceptionHandler`, without calling their handlers. And an exception thrown in an OS/2 exception handler could not unwind past it. ADR-0103's follow-up, "OS/2 exceptions running C++ destructors", was still open.

## Decision

1. **`DosUnwindException(phandler, pTargetIP, pERepRec)`** (`seh.rs`, `ApiResult::Unwind`, `FrameKind::UnwindHandler`):
   - The handlers above `phandler` are called one at a time, newest first, as guest calls with the four handler arguments. Each record leaves the chain when its handler returns.
   - `phandler` stays. `END_OF_CHAIN` or `UNWIND_ALL` (0) unwinds the whole chain. A `phandler` not on the chain is refused with `ERROR_INVALID_PARAMETER`, and nothing is unwound.
   - **The report record:** `pERepRec` with `EH_UNWINDING` added to its flags, or one Warpine makes (`XCPT_UNWIND`, `EH_UNWINDING`, the caller's return address).
   - **The context record:** the caller's state, with EIP at the return address.
   - **Afterwards** the thread goes on at `pTargetIP` as if the call had returned there (ESP after the return, the caller's EBX / ESI / EDI / EBP, EAX = `NO_ERROR`). With a NULL `pTargetIP` the call simply returns.
2. **`DosRaiseException` fills in `ExceptionAddress`** with its return address.
3. **Handler calls the guest leaves.** A handler that `longjmp`s out never returns to Warpine.
   - At a later callback return, exception and unwind handler frames whose stack the trap's ESP lies above are dropped and their records freed (`drop_abandoned_handler_frames`).
   - Exception handlers are called with their own return address, `XCPT_RET_TRAP` (`0x010003FD`, item 7).
4. **The low 64 KB are not present** for 32-bit threads (`paging.rs`, the "low table"). A NULL pointer, or one just past it, raises `XCPT_ACCESS_VIOLATION`, as on OS/2. A page fault's `ExceptionInfo` is now `[XCPT_READ_ACCESS or XCPT_WRITE_ACCESS, the address]` (CR2), no longer `[0, EIP]`. The paging block grows to four pages: directory, TLM table, low table, thread-local page.
5. **FS through a window.**
   - `FS_WINDOW` is the page after the system area. Each thread's TLM table maps it to the thread's own TIB page, and selector `0x18` has it as its base. So reloading FS keeps the thread's TIB, as OS/2's per-thread TIB selector does.
   - `DosGetInfoBlocks` returns the TIB itself, from a thread-local the vCPU thread sets (`guest_mem::thread_tib`); the integrity check of a thread's tables uses it too.
   - Programs see the same TIB through FS and through `ptib`.
6. **The SDK:**
   - **`setjmp` / `longjmp`** (`setjmp.h`, `libc/setjmp.c`): `longjmp` unwinds the handlers registered since `setjmp` with `DosUnwindException`, as Open Watcom's does.
   - **The C++ unwinder** unwinds the records of the frames it leaves with `DosUnwindException`, so their handlers see `EH_UNWINDING`.
   - **`ERR` is a pointer** to `_ERR`, as in Open Watcom's header, so `(ERR)handler` casts compile. `PERR` stays the same pointer.
   - **New constants:** `END_OF_CHAIN`, `UNWIND_ALL`, `XCPT_UNWIND`, `XCPT_INVALID_UNWIND_TARGET`, `XCPT_*_ACCESS`, `XCPT_FATAL_EXCEPTION`, `XCPT_RET_TRAP`.
7. **C++ exceptions from OS/2 traps** (GCC and clang):
   - An OS/2 handler may `throw`. The SDK's unwinder recognises `XCPT_RET_TRAP` as the handler's return address and steps into the context the handler was called for, read from its CONTEXTRECORD argument (`enter_trapped_context`).
   - That frame is a signal frame for hardware and asynchronous codes: its IP is the instruction itself, not a return address (`_Unwind_GetIPInfo`).
   - The frames that trapped are unwound and their destructors run, up to a catch above them. The handler's own record is unwound like any other.
   - The personality lets an exception pass a trapping instruction that lies outside every call site.
   - **Unwind tables:** C++ code (`OS2_CXXFLAGS`) and the SDK's C library are built with `-fasynchronous-unwind-tables`. clang emits tables only where it thinks an exception can pass, which a trap can be. Later, every program's C code is too (`OS2_BASEFLAGS`, and `build_sdk.sh`), as GCC and clang do for Linux by default: a trap in C code under a C++ `catch`, or an exception passing through C frames, unwinds instead of ending in `std::terminate`; a program without C++ only carries the tables.

## Warpine's choices (unverified)

- **Unlink order:** a record leaves the chain after its handler returns, so the handler still sees its own record at the head. A handler that took itself off is not unlinked twice.
- **Handler results:** a handler's return value during an unwind is ignored, and nested unwinds (`EH_NESTED_CALL`) are not marked.
- **`EH_EXIT_UNWIND`** is never set by `DosUnwindException`. It is left for a thread that ends.
- **Refused targets:** a `phandler` not on the chain is refused with `ERROR_INVALID_PARAMETER`, and the call returns without unwinding (Windows NT would raise `XCPT_INVALID_UNWIND_TARGET`).
- **Allocations:** Warpine's report record (`XCPT_UNWIND`) and the context record are allocated per call and freed at its end.
- **Abandoned frames:** the frames of handlers left by `longjmp` are dropped only at a later callback return whose stack lies above them; until then their records stay allocated.
- **The FS window:** at `SYSTEM_AREA_BASE + SYSTEM_AREA_SIZE`, with a 4 KB limit. A thread running unpaged, when its tables could not be allocated, keeps a direct FS base, so reloading FS still loses its TIB there.
- **Which codes are signal frames:** the hardware exceptions and `XCPT_SIGNAL`. A program that raises `XCPT_ACCESS_VIOLATION` itself with `DosRaiseException` gets a frame treated as trapping.
- **Uncovered trapping instructions:** one outside every call site lets the exception pass that frame without its cleanups. Code built without GCC's `-fnon-call-exceptions` gets no cleanups for the instruction that traps. clang does not honour that flag: in `cxx_test` the trapping call goes through a pointer, so clang cannot prove the caller's cleanup unneeded.

## Consequences

- **Open Watcom C++ programs** run their destructors when exceptions pass, rethrows work, and frames with OS/2 handlers between thrower and catcher are unwound. `wcxx_throw_test` passes 12 of 12; on the old `DosUnwindException` it lost every destructor.
- **NULL dereferences trap:** a program that read or wrote near address 0 and went on now gets `XCPT_ACCESS_VIOLATION`, and without a handler it ends with a crash report.
- **Open Watcom's `longjmp`** keeps the TIB and the exception chain, and unwinds the handlers registered since `setjmp`.
- **The SDK gains `setjmp` / `longjmp`,** and C++ code can turn traps into exceptions with destructors run. `cxx_test` checks it three times over (40 checks).
- **Samples:**
  - `xcpt_unwind_test` (28 checks, Open Watcom, GCC and clang);
  - `wcxx_throw_test` (12 checks, Open Watcom C++ only, skipped by `build_sdk.sh`);
  - `rexx_api_test` no longer calls `strlen(NULL)`.
- **Tests:**
  - `seh.rs`: unwind steps, whole-chain unwinds, given and stray records, records without handlers;
  - `paging.rs`: the low table, the FS window;
  - `vcpu.rs`: abandoned frames.
