# ADR-0103: A C++ runtime for the SDK: the Itanium C++ ABI, a DWARF unwinder, and OS/2 exception handlers

- **Status:** Accepted; builds on ADR-0095, ADR-0096, ADR-0101; amended by ADR-0107 (exceptions across modules)
- **Date:** 2026-11-21
- **Sources:**
  - The Itanium C++ ABI: run-time type information (section 2.9), the exception-handling ABI (levels I and II), `__cxa_atexit`, the guard variables, `__dynamic_cast`.
  - The System V i386 ABI and the DWARF 4 standard, section 6.4 (call-frame information and its instructions), and DWARF expressions (section 2.5).
  - The Linux Standard Base core specification: `.eh_frame`, its CIE and FDE records, the augmentation strings and the `DW_EH_PE_*` pointer encodings.
  - GCC's documented LSDA layout (header, call-site table, action table, type table).
  - IBM's *Control Program Reference*: exception handler registration (`DosSetExceptionHandler` / `DosUnsetExceptionHandler`) and the chain at `fs:[0]`.

## Context

M4's second part: C++ programs from g++ and clang++. Both compilers emit code for the Itanium C++ ABI, which needs:

- a runtime: `operator new` and `delete`, `__cxa_*`, the `type_info` classes and the fundamental types' type information, `__dynamic_cast`, the personality routine;
- an unwinder reading the call-frame information the compilers emit in `.eh_frame`.

`lx-link` dropped `.eh_frame`. OS/2 has its own exception mechanism: handlers registered in records on the stack, chained from the thread's TIB at `fs:[0]`.

## Decision

1. **The runtime is the SDK's own** (`sdk/libcxx/`, built into `libos2cxx.a` with g++ or clang++), written from the ABI documents:
   - **`unwind.c`:** the base unwinder (`_Unwind_RaiseException`, `_Unwind_Resume`, `_Unwind_Resume_or_Rethrow`, `_Unwind_Backtrace`, the context accessors).
     - It finds a frame's FDE by scanning `.eh_frame` and runs the CIE's and FDE's call-frame instructions.
     - It evaluates the DWARF expressions GCC emits for frames that realign the stack (`DW_CFA_def_cfa_expression`, `DW_CFA_expression`).
     - It unwinds in two phases, as the ABI describes.
     - It installs a landing pad with the call's `DW_CFA_GNU_args_size` removed from the stack.
   - **`cxa_exception.cc`:** `__cxa_allocate_exception` / `__cxa_throw` / `__cxa_begin_catch` / `__cxa_end_catch` / `__cxa_rethrow` and friends; `__gxx_personality_v0`, reading the LSDA (call sites, actions, catch types, exception specifications); `std::terminate`; the guard functions (a byte for "done", a byte for "in progress"); `__cxa_pure_virtual`.
   - **`typeinfo.cc`:** `std::type_info` and the ABI's `type_info` classes.
     - Defining `__fundamental_type_info`'s destructor makes the compilers emit the fundamental types' `type_info` objects there.
     - Catch matching: the same type; a public base class by reference or by pointer, including virtual bases; qualification conversions; `void *`; `nullptr`.
     - `__dynamic_cast`: down, across and to `void *`.
   - **`new.cc`, `stdexcept.cc`:** `operator new` and `delete` (plain, array, nothrow, sized, over-aligned; the new handler; `std::bad_alloc`), and the standard exception classes.
   - **C++ headers** (`sdk/include/c++/`): `<new>`, `<exception>`, `<typeinfo>`, `<initializer_list>`, `<utility>`, `<stdexcept>`, and the `<c…>` wrappers of the C headers.
     - There is no standard template library.
2. **The C runtime's part:**
   - `__cxa_atexit` and `__cxa_finalize`: `exit()` runs the destructors of statics and the `atexit` functions newest first. A DLL's termination in `dll0.o` runs its own, by its `__dso_handle`.
   - The thread-local memory `crt0` and `dll0` allocate grows from 1 word to 3: `errno`, then the caught exceptions and the uncaught count for the C++ runtime.
   - `<stdint.h>` is the SDK's own, from the compilers' predefined types.
3. **`lx-link` keeps the unwind tables.**
   - `.eh_frame` is parsed into records.
   - A kept function's FDE keeps its LSDA (`.gcc_except_table`) and its CIE's personality routine alive under `--gc-sections`.
   - Each object's `.eh_frame` is rebuilt from the FDEs of kept functions and the CIEs they use, CIE pointers recomputed, relocations moved along.
   - The tables are placed together in the data object between `__EH_FRAME_BEGIN__` and `__EH_FRAME_END__`, followed by a zero terminator.
4. **OS/2 exception handlers.**
   - Before control moves to a landing pad, every handler registration below the landing pad's stack pointer, which lives in a frame being left, is removed with `DosUnsetExceptionHandler`. This is what unwinding through those frames on OS/2 would do.
   - A C++ exception does not go through OS/2's exception dispatch: handlers registered with `DosSetExceptionHandler` are not called for it.
5. **Compiling C++:**
   - `os2.mk` gives `OS2_CXX` (g++ beside gcc, clang++ beside clang) and `OS2_CXXFLAGS`: the SDK's C++ headers first, `-std=gnu++17 -fexceptions -frtti`.
   - Hosted, not freestanding: a freestanding clang++ mangles `main`.
   - Programs link `libos2cxx.a` before `libos2c.a`.

## Warpine's choices (unverified)

- **One runtime per module:** each EXE and DLL links its own copy and unwinds only its own frames. An exception must not leave the module that threw it. (ADR-0107 lifts this: the copies find each other's frames and share the per-thread state.)
- **API callbacks:** an exception must not cross a call into Warpine either, such as a window procedure called from `WinDispatchMsg`: the unwinder ends there and the program terminates.
- **OS/2 exceptions do not run C++ destructors:** traps and `DosRaiseException` go through OS/2's handlers only.
- **Foreign exceptions:** an exception class not thrown by this runtime is caught only by `catch (...)`.
- **Exception specifications:** a dynamic exception specification that rejects the exception terminates the program (`__cxa_call_unexpected` does not call an unexpected handler).
- **`std::terminate`'s default** writes "terminate called: an exception was not caught" and calls `abort()` (exit code 3).
- **`__dynamic_cast`** with several candidate subobjects picks the one that contains the source subobject; it does not use the ABI's `src2dst` hint.

## Consequences

- **C++ programs from g++ and clang++ run on Warpine:**
  - static construction and destruction;
  - `new` / `delete`;
  - virtual functions, RTTI and `dynamic_cast`;
  - exceptions through frames with destructors, rethrow, `std::bad_alloc` and `std::bad_cast`;
  - static locals;
  - exceptions on several threads.

  This holds for the host compilers and the pinned ones (g++ 14.2, clang++ 19.1.7).
- **`samples/cxx_test`:** 27 checks, with g++ and clang++. These include an exception passing a frame with an OS/2 handler registration, which leaves the chain, and an uncaught exception ending the program with exit code 3.
- **C programs are unaffected:** they are compiled without unwind tables, so they have no `.eh_frame`.
- **Tests:**
  - `lx_link.rs`: `.eh_frame` pruning and placement, a malformed record;
  - `tests/integration.rs`: `test_cxx_from_gcc_and_clang`.
