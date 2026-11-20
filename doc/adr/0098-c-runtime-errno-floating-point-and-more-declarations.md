# ADR-0098: A per-thread errno, exact floating point in printf, and most of the def declared

- **Status:** Accepted; amends ADR-0096 (its runtime) and ADR-0097 (what `pm.h` declares)
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosAllocThreadLocalMemory`, the `DosProtect*` calls.
  - IBM's *Presentation Manager Programming Reference*: the window-manager and graphics calls and structures added here.
  - The C standard (C11 7.21.6.1): the `e`, `f` and `g` conversions; IEEE 754 binary64.
  - Warpine's code: `paging.rs` (each thread's thread-local page starts zeroed), `pm_bitmap.rs` (`BITMAPINFOHEADER2` offsets), and the constants in `src/`.

## Context

ADR-0096's runtime left three things open: `errno` was one variable for the whole process, `printf` could not format floating point, and the headers declared 430 of the def's calls. A per-process `errno` is wrong as soon as a program has two threads. `printf` without `%f` is the first thing a ported program misses. Every undeclared call needs a hand-written prototype in the program.

## Decision

1. **`errno` is a word of thread-local memory:**
   - `crt0` takes one word with `DosAllocThreadLocalMemory` before anything else runs;
   - `errno` is `(*__errno_location())`, which returns that word's address (the same in every thread, each thread seeing its own word);
   - every thread starts with its thread-local page zeroed, so a new thread's `errno` is 0;
   - before the word exists, and if it cannot be had, a process-wide fallback is used.
2. **Floating point in `printf`** (`e E f F g G`, with every flag, width and precision; `L` takes a `long double` and rounds it to a double):
   - **Exact digits** (`dtoa.c`): a finite double is m × 2^e. Its integer part is m shifted and written out in base 10, and each fraction digit is the top of the fraction times 10. Both use word arrays, at most 1074 fraction bits. The expansion is exact and uses no floating-point instruction.
   - **One rounding:** at the requested precision, ties to even, on the exact digits. So `%.2f` of 2.675 prints `2.67`, and `%.17g` of 0.1 prints `0.10000000000000001`.
   - **Limits:** `inf` / `nan` (upper case for the capital conversions), and a precision capped at 800 digits so the buffers stay bounded.
   - **The buffers are on the stack,** so threads can print at the same time.
3. **More of the def declared:** 549 calls now, of the 579 the covered modules export.
   - **Control Program:** the `DosProtect*` family, with `FHLOCK`.
   - **Window manager:** window enumeration (`HENUM`), accelerator tables (`ACCEL`, `ACCELTABLE`, `AF_*`), hooks (`HK_*`), `WinGetDlgMsg`, error information (`ERRINFO`), the clipboard's owner and viewer, codepage-aware comparison and stepping (`WCS_*`), cursor and pointer information (`CURSORINFO`, `POINTERINFO`), pointers built from bitmaps, update regions.
   - **Graphics:** regions (`CRGN_*`, `RGN_*`, `PRGN_*`, `RGNRECT`), paths and curves, polygons, the transforms (`MATRIXLF`, `TRANSFORM_*`, `CVTC_*`), page viewport, graphics field, viewing limits, the remaining character attributes and text alignment (`TA_*`), the width table and string positions, font queries and loading, and bitmaps built in memory (`BITMAPINFOHEADER`, `BITMAPINFOHEADER2`, `BITMAPINFO2`, `RGB2`, `CBM_INIT`, `BCA_*`).
   - **Left out** (30 entries): calls only for 16-bit programs, `DosQuerySysState`, `DosDebug`, `DosFSCtl`, the raw pipe calls, DDE, `WinTrackRect`, the attribute bundles, `DevEscape`, `DevPostDeviceModes`, `WinSetSysColors`, `WinLoadLibrary` / `WinLoadProcedure`, `WinRegisterUserDatatype` / `Msg`, `WinMakePoints` / `WinMakeRect`, `WinSetErrorInfo`, `WinCpTranslateChar`. Their structures or their purpose would be guessed.

   The new constants go through `check-headers` like the others: 896 of the 1059 constants are compared with Warpine's.

## Warpine's choices (unverified)

- **`L` conversions round to a double:** `long double` is not formatted at its own precision.
- **The precision cap:** a precision over 800 is taken as 800.
- **The fallback `errno`:** one process-wide word, when no thread-local word can be had.

## Consequences

- **A multithreaded C program** keeps each thread's `errno` apart: `c_sdk_test` checks that a new thread starts at 0 and that its value does not reach the thread that started it.
- **`printf` output** for doubles is what a correctly rounding C library prints. 29 cases, checked against Python's `%` formatting, pass with GCC and with clang, as do infinities, NaN, negative zero and `long double` arguments.
- **Ports need fewer prototypes:**
  - 549 of the 579 entries are declared;
  - `c_pm_test` uses regions, transforms, text alignment, window enumeration, accelerator tables and string comparison;
  - `c_sdk_test` uses the `DosProtect*` calls, and checks that a wrong lock ID is refused.
- **Tests:**
  - `samples/c_sdk_test`: 56 checks;
  - `samples/c_pm_test`: 41 checks;
  - both pass built by GCC and by clang (`test_c_sdk_from_gcc_and_clang`, `gui_c_pm_from_gcc_and_clang`).
