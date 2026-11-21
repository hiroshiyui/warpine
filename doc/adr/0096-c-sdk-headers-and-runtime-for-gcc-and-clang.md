# ADR-0096: A C SDK for GCC and clang: os2.h and a small runtime

- **Status:** Accepted; builds on ADR-0095; amended by ADR-0097 (PM headers; `check-headers` compares every integer constant) and ADR-0098 (`errno` per thread, floating point in `printf`); ADR-0110 adds `setjmp` / `longjmp` and unwind tables for C++ and the C library
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Reference*: the prototypes, structures, flags and error codes of the Dos* and Que* calls, and of the Vio*, Kbd* and Mou* subsystem calls.
  - The ISO C standard (C11), for the library functions the runtime provides.
  - The GCC and clang manuals: `-ffreestanding`, `-nostdinc`, `-mincoming-stack-boundary`, `-mstack-alignment`, asm labels, `stdcall`.
  - Warpine's own code, for layouts it already fills: `CTX_*` (`constants.rs`), `MOUEVENTINFO_SIZE`, `KBDKEYINFO` (`kbdcalls.rs`), `VIOCONFIGINFO` (`viocalls.rs`), and the argument bytes Warpine pops for each subsystem call.
  - `doc/TODOs.md` "Development toolchain" (M2).

## Context

With ADR-0095 an unmodified GCC or clang builds OS/2 programs, but each program had to declare its own calls (`samples/c_hello/os2min.h`) and carry its own start-up code. M2 makes that an SDK: the familiar headers, imports that resolve through the one ordinal table, and a C library small enough to own. Three things stood in the way:
- **Two conventions.** The 32-bit system calls are cdecl. The VIO, KBD and MOU calls are Pascal: arguments pushed left to right, removed by the callee. Neither compiler has a Pascal convention.
- **No toolkit text.** The headers cannot come from IBM's toolkit or Open Watcom's; they are written from the reference.
- **The stack.** OS/2 keeps the stack 4-byte aligned; both compilers assume 16 bytes on i386 by default, and use aligned SSE stores on the strength of it.

## Decision

1. **Where it lives:** `sdk/` in the repository.
   - `sdk/include/os2.h` and `sdk/include/os2/{os2def,dos,errors,sub}.h`: the API.
   - `sdk/include/{string,stdlib,stdio,ctype,errno,assert,limits}.h` and `sdk/libc/*.c`: the runtime.
   - `sdk/os2.mk`: the flags, for a program's Makefile to include.
   - `sdk/Makefile`: builds `crt0.o` and `libos2c.a` per compiler, into `sdk/build/<compiler>/`.
2. **`os2.h` selects with `INCL_*`** as the familiar headers do:
   - `INCL_BASE` means `INCL_DOS` + `INCL_SUB` + `INCL_DOSERRORS`;
   - any `INCL_DOS*` name selects all of `dos.h`;
   - `INCL_VIO` / `INCL_KBD` / `INCL_MOU` select their parts of `sub.h`.

   The Presentation Manager is not declared: PMWIN and PMGPI are not in `targets/os2api.def` yet, so `lx-link` could not resolve them.
3. **Conventions:**
   - `APIENTRY` (and `_System`, `EXPENTRY`) is `__attribute__((cdecl))`.
   - **Pascal calls:** each is declared through `OS2_PASCAL_API`. That makes the import (`_os2p_Name`, linked as `Name` by an asm label) stdcall, with its arguments reversed and every scalar widened to 32 bits: stdcall also has the callee remove the arguments, and the reversed order gives the same stack. An always-inline function keeps the documented name and order.
   - **The inline function's own symbol:** the wrapper is linked as `_os2i_Name`. Clang names a declaration by its asm label, so a wrapper named `Name` shared the import's symbol, called itself with the wrong convention, and clang dropped the code as unreachable.
4. **Imports come from `targets/os2api.def`:** `lx-link` already resolves names through it. `gen_api check-headers` checks that every declared call is in it under its convention (Pascal exactly for VIOCALLS / KBDCALLS / MOUCALLS). It also checks that every `ERROR_*` the headers define equals Warpine's constant of that name. A unit test runs the check on the real tree.
5. **The flags** (`OS2_CFLAGS`):
   - `-m32 -ffreestanding -fno-pic -fno-stack-protector -fno-asynchronous-unwind-tables -ffunction-sections -fdata-sections`;
   - `-nostdinc` with the SDK's include directory and the compiler's own (`stddef.h`, `stdint.h`, `stdarg.h`);
   - the stack: `-mincoming-stack-boundary=2` for GCC, `--target=i386-unknown-none -mstack-alignment=4` for clang. A function that needs more alignment realigns its own frame.
6. **The runtime:**
   - **`crt0.o`:** `_start` splits the command line into `argv` (blanks separate, double quotes group, `\"` is a quote) and the environment block into `envp`. It runs the preinit and init arrays, then `main(argc, argv, envp)`, and leaves through `exit`.
   - **`exit`:** runs the `atexit` functions (32 at most), the fini arrays, closes the streams, then `DosExit(EXIT_PROCESS)`. `abort` writes a line to standard error and ends with exit code 3.
   - **The heap:** a first-fit free list in address order. Freed blocks merge with free neighbours; 16-byte headers keep blocks 16-byte aligned. Arenas of 256 KB come from `DosAllocMem`; requests of 128 KB or more get their own object, which `free` releases. A spin lock that yields with `DosSleep(0)` serialises it across threads.
   - **Streams:** on OS/2 handles. Output goes to `DosWrite` at the end of each call; input is read ahead `BUFSIZ` bytes at a time; `fseek` and writes drop the read-ahead.
   - **`printf`:** the integer, character, string and pointer conversions with all flags, widths, precisions and lengths; no floating point.
   - **Also:** `string.h` (with `stricmp` / `strnicmp`), conversions, `qsort` (Shell sort), `bsearch`, `rand`, `getenv`, a C-locale `ctype.h`. The 64-bit division helpers GCC and clang call (`__udivdi3` …) are included, since nothing else provides them.
7. **The sample:** `samples/c_sdk_test` builds with either compiler through `os2.mk` and checks 51 things:
   - structure layouts, at compile time;
   - `argv`, `envp` and `getenv`;
   - strings, `printf`, conversions, 64-bit division, sorting;
   - the heap under stress;
   - streams on a file;
   - two threads under a mutex, and a thread's stack realigned;
   - an exception handler that continues;
   - VIO calls in Pascal order, including a hand-written Pascal call that checks the callee removed its arguments.

## Warpine's choices (unverified)

- **Strings are `char`:** `PSZ` / `PCSZ` / `PCH` are plain `char` pointers, so string literals need no casts. The familiar headers use `unsigned char`.
- **The subsystem structures are packed to 2 bytes:** taken from the offsets Warpine fills (`KBDKEYINFO.time` at 6, `MOUEVENTINFO` 10 bytes, `VIOCONFIGINFO.cbMemory` at 6).
- **The find buffers are packed to 1 byte:** `FILEFINDBUF3.achName` is at 29.
- **`FSALLOCATE`** has natural alignment (20 bytes); the call returns 18.
- **`CONTEXTRECORD`** keeps its floating-point area as 0x88 bytes, which is Warpine's layout; the register offsets are Warpine's `CTX_*`. *(Superseded by ADR-0101: that layout was wrong; Warpine and the SDK now follow IBM's, `ctx_env[7]` and eight packed `FPREG`s, `ctx_SegGs` at 0x70.)*
- **`errno` is one per process,** not per thread.
- **The exit code of `abort`** is 3.
- **No stream buffers output:** `fflush` does nothing.
- **The heap's sizes** (256 KB arenas, 128 KB for its own object) and its spin lock.

## Consequences

- **Both compilers:** a C program built with the SDK runs the same when built by GCC 16 or clang 21: `c_sdk_test` passes all 51 checks with each.
- **Header drift is caught:**
  - a declaration with a typo, or one whose call is not in the def, fails `cargo test`;
  - so does a Pascal call declared cdecl, or the reverse, and an error code that disagrees with Warpine's.
- **Coverage:**
  - the headers declare 194 calls, each resolvable;
  - of the def's Control Program and subsystem entries, 23 are not declared yet: 16-bit-only entries and the `DosProtect*` family among them;
  - extended attributes (`EAOP2`) are opaque.
- **`samples/c_hello` stays** as the smallest program, without the SDK (ADR-0095).
- **Next:**
  - PM headers, after PMWIN and PMGPI get their def entries;
  - M3: both compilers on every C sample, pinned;
  - M4: DWARF and C++.
- **Tests:**
  - `gen_api`, 4 new unit tests: declarations of both conventions found, each kind of mistake named, error codes compared, and the real tree checked;
  - `tests/integration.rs`: `test_c_sdk_from_gcc_and_clang`.
