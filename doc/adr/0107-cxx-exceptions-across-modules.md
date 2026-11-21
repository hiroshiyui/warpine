# ADR-0107: C++ exceptions across modules: registered unwind tables and a shared runtime block

- **Status:** Accepted; amends ADR-0103 ("one runtime per module ... an exception must not leave the module that threw it")
- **Date:** 2026-11-21
- **Sources:**
  - The Itanium C++ ABI's exception-handling chapter: `_Unwind_DeleteException` and the exception's cleanup function, which let a runtime free an exception another runtime allocated; the per-thread caught-exception stack and uncaught count.
  - GCC's documentation of frame registration (`__register_frame_info`): the model of a module registering its `.eh_frame` with the unwinder.
  - IBM's *Control Program Programming Reference*: `DosAllocThreadLocalMemory` (32 words).

## Context

ADR-0103 gave each module (the program and every DLL) its own copy of the SDK's C++ runtime. Each unwinder scanned only its own module's `.eh_frame`, so an exception could not leave the module that threw it:
- a DLL throwing to the program it serves ended in `std::terminate`;
- so did a program's exception passing through a DLL's callback.

Three things kept exceptions inside a module:
1. **Finding frames:** an unwinder found no FDE for another module's code.
2. **Freeing:** `__cxa_end_catch` freed the exception with its own module's `free`, but another module's heap may have allocated it.
3. **Counting:** the caught-exception stack and the uncaught count lived in each module's own thread-local words. A cross-module catch left the thrower's count at 1.

## Decision

1. **Modules register their unwind tables with Warpine** (new calls of Warpine's own `WARPINE` module, declared in the SDK's `os2/warpine.h`):
   - `WrpRegisterUnwindTables(pBegin)` (WARPINE.7) records that the tables at `pBegin` (`__EH_FRAME_BEGIN__`, ended by lx-link's zero word) belong to the module whose object holds `pBegin`;
   - `WrpDeregisterUnwindTables(pBegin)` (WARPINE.8) forgets them, and so does freeing the DLL;
   - `WrpQueryUnwindTables(pCode, ppBegin)` (WARPINE.9) answers with the tables of the module whose code holds `pCode`.
   - The unwinder registers its module's tables in a constructor and deregisters them in a destructor.
   - It scans its own tables first, then those of the module that holds the PC.
   - The personality routine is the one the FDE's CIE names, in that module.
2. **Exceptions are freed by the runtime that made them.** `__cxa_end_catch` deletes an exception with `_Unwind_DeleteException`, which calls the cleanup function the throwing runtime stored in it.
3. **A runtime block per thread, shared by every module** (`WrpQueryRuntimeBlock(ppBlock)`, WARPINE.10):
   - 64 bytes at `TLM_PAGE + 0x800`, the thread-local page's part past the 32 words `DosAllocThreadLocalMemory` hands out;
   - the same address on every thread, mapped to each thread's own page, and zero when the thread starts;
   - words 0 and 1 hold the C++ runtime's caught-exception stack and uncaught count, so every module's runtime sees the same ones; the others are reserved.
   - The C runtime's thread-local words shrink back to one (`errno`).
4. **Type matching** already compares `type_info` names (ADR-0103), so a catch in one module matches a type whose `type_info` another module emitted.

## Warpine's choices (unverified)

- **The `WARPINE` calls and the runtime block** are Warpine's own; programs that use them (any C++ program from the SDK) run on Warpine only. This was already so for `WrpStartProcess`.
- **Registration by address:** a module's tables are found through the module holding the PC: a DLL's objects (`DllManager::find_object`), or the program's.
- **The shared block assumes one C++ runtime layout** across the modules of a process. Every module built with the SDK has it; a module with another C++ runtime must not share exceptions with them.
- **Still not crossed:** calls into Warpine. An exception thrown in a window procedure that `WinDispatchMsg` called ends there, and the program terminates.

## Consequences

- Exceptions pass between a program and its DLLs both ways, through frames with destructors, and with rethrows. `std::uncaught_exceptions()` agrees in every module.
- **`samples/cxx_test`** has a C++ DLL, `cxxdll.dll`, with its own runtime. Eight new checks (35 in all) pass with g++ and clang++:
  - thrown in the DLL and caught by type and by a standard base;
  - thrown in the program and caught in the DLL;
  - passing a DLL frame whose destructor runs;
  - rethrown in the DLL;
  - 200 throws freed by their own runtime;
  - both runtimes' uncaught counts.
- **`samples/build_sdk.sh`** skips `cxx_test` and `c_msg_test`, which have their own Makefiles and tests; `cxx_test` had been reported as a failure.
- **Tests:**
  - `warpine_api.rs`: registration, lookup by code address, deregistration, freeing a DLL, the runtime block;
  - `tests/integration.rs`: `test_cxx_from_gcc_and_clang`.
