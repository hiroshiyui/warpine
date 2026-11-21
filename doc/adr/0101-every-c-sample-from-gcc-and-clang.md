# ADR-0101: Every C sample from GCC and clang: the whole SDK, DLLs from lx-link, pinned compilers

- **Status:** Accepted; builds on ADR-0095–0100; its `DosGetMessage` choice is amended by ADR-0105
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program*, *Presentation Manager* and *Multimedia Programming References*, the *REXX Reference*: the calls, structures and constants declared.
  - The LX format: module flags (library, `INITINSTANCE`, `TERMINSTANCE`), the entry table, the resident and non-resident name tables.
  - The oracle: the Open Watcom builds of the samples and Open Watcom's headers. Structure layouts and constant values were compared by compiling programs, never by copying text.
  - Debian's snapshot archive (`snapshot.debian.org`) and Docker Hub's `debian` images, for the pinned compilers.

## Context

M3 of the development toolchain plan: GCC and clang build every C sample, with pinned compilers, while the Open Watcom binaries stay the regression baseline.

After ADR-0100, 37 of the 117 samples compiled with the SDK. The others needed declarations the headers left out, structures of the standard controls, DLLs that `lx-link` could not make, and the C runtime's `time()`, `sscanf` and friends. The samples also carry code only Open Watcom accepts: `#pragma aux`, `_asm`, 16-bit `_Pascal` declarations, `#pragma pack( __push, n )`.

## Decision

1. **`os2.h` declares everything.**
   - `INCL_*` are accepted and select nothing.
   - The headers are small, and a program that forgot an `INCL_` gets the declaration rather than an implicit one.
2. **New headers and declarations:**
   - `os2/pmctl.h`: the controls' messages, flags and structures (containers, notebooks, sliders, spin buttons, value sets, scroll bars, combination boxes, the MLE, owner drawing), DDE, `WinTrackRect`, the attribute bundles, `DevEscape`, the spooler's queries, and more window calls.
     - Its constants are generated from Warpine's own values; `gen_api check-headers` keeps them equal.
   - In `os2/dos.h`: NLS (`DosQueryCtryInfo`, `DosQueryDBCSEnv`, `DosMapCase`, `DosQueryCollate`), MSG (`DosGetMessage`, `DosInsertMessage`, `DosPutMessage`, `DosQueryMessageCP`), `PIPEINFO`, the 64-bit file structures, and the OS/2 1.x calls `DosGetInfoSeg` and `DosSetFileMode`.
   - `rexxsaa.h`, `os2me.h` (the MCI calls and the basic command structures), `pmerr.h`, `time.h`, and, in `stdio.h`, `sscanf` and `setvbuf`.
   - **`DosGetMessage`** goes through a stub, `__warpine_DosGetMessage` (`libc/msgstub.c`). The import takes the address of a message segment before the documented arguments and removes it itself. The stub puts a null one under the return address.
3. **Structure layouts are held to Open Watcom's.**
   - `tests/sdk_layout/layout.c` prints the size of 142 structures and the offset of each of their fields, 1022 lines. It is built by Open Watcom with its headers, and by GCC and clang with the SDK's, and the outputs must be equal.
   - The comparison found and fixed:
     - **SDK:** `CNRDRAWITEMINFO` (two fields, not three); `GRIDINFO` (two more fields); the find buffers and the EA structures padded to 4 bytes; `FONTMETRICS` (`FamilyNameAtom`, `FaceNameAtom`, `PANOSE`); `VIOCONFIGINFO` (the whole structure).
     - **Warpine:** see Consequences.
   - `ERRINFO` keeps the Warp 4.5 Toolkit's ULONG fields (ADR-0086) and is left out of the comparison.
4. **`lx-link` makes DLLs and links against them:**
   - **Making:**
     - `--dll` (or an output ending in `.dll`), `--name`, `--export NAME[.ORDINAL]`, `--initinstance` / `--terminstance`;
     - module flags 0x8000 with 0x4 and 0x40000000 for the instance flags;
     - an entry table with type-0 bundles for unused ordinals;
     - exports in the resident name table, the module name as its entry 0;
     - no stack object;
     - `_DLL_InitTerm` as the entry, when defined.
   - **Linking against:** a `.dll` input's exports (resident and non-resident names) become imports by ordinal from its module.
   - **Fixups:** offset (0x07) and self-relative (0x08) records, with an additive for imports.
5. **`dll0.o`**, the SDK's DLL start-up: a weak `_DLL_InitTerm` that sets up the DLL's copy of the runtime, runs its constructors and calls `__dll_initialize` (the hook Open Watcom's runtime calls) on attach, and runs `__dll_terminate`, the C++ destructors (ADR-0103) and the destructors on detach.
6. **`samples/build_sdk.sh [gcc|clang] [sample...]`** builds every C sample into `samples/build/<cc>/<sample>/` under the names Open Watcom gives, with the sample's data files.
   - A sample is one `.c` and its `.rc`. The DLL samples, the samples built several times with `-D`, and the SOM samples have their own rules.
   - Not built: the 16-bit `ne_hello`, the SDK's own samples, the Rust guest, Open Watcom's `shapes`, and `som_test`.
7. **Samples are changed only behind `#ifdef __WATCOMC__`**, with an equivalent for the other compilers (16-bit declarations, `#pragma aux`, `_asm`), or where they were wrong for every compiler (see Consequences).
   - `os2.mk` defines `_System`, `__syscall` and `_Optlink` as nothing (all cdecl), also for sources that include no header.
8. **The integration tests run either build:**
   - `WARPINE_SAMPLES=gcc` (`clang`, `gcc-pinned`, `clang-pinned`) maps `samples/<name>/<file>.exe|.dll` to `samples/build/<tag>/<name>/` in `sample_path`, for every program run, copied or checked for existence;
   - Open Watcom stays the default.
9. **Pinned compilers:**
   - `vendor/setup_compilers.sh` builds `localhost/warpine-cc:trixie-20261115T000000Z` from `debian@sha256:7378260c…` with gcc, g++, clang, binutils and make from `snapshot.debian.org` at that timestamp. apt checks them against the archive keys in the image.
   - `vendor/pinned/{gcc,g++,clang,clang++}` run them with no network, the repository mounted at its own path, and the caller's user id.
   - `CC_CMD=vendor/pinned/gcc OUT_TAG=gcc-pinned samples/build_sdk.sh gcc` builds the samples with them.

## Warpine's choices (unverified)

- **`DosGetMessage`:** the message segment the import skips is passed as null by the stub; programs that bind messages into their executable are not supported. (ADR-0105 binds Warpine's own message files and passes their segment.)
- **The layouts** follow Open Watcom's headers where IBM's reference does not give sizes:
  - the find buffers and EA structures padded to 4 bytes, but the 64-bit find buffers packed;
  - `VIOCONFIGINFO` packed to 2.
- **`lx-link` exports:** an export with no ordinal gets the lowest free one; every export goes in the resident name table.
- **`build_sdk.sh` DLLs** are `INITINSTANCE` and `TERMINSTANCE`.
- **`INCL_*`** select nothing, so a program can see names its OS/2 toolkit build would not.

## Consequences

- **116 samples build with GCC and with clang.**
  - Both host compilers (GCC 16.2, clang 21.1) and the pinned ones (GCC 14.2, clang 19.1.7) build them.
  - The whole integration suite passes against the GCC and the clang builds, as it does against Open Watcom's.
- **Warpine bugs the comparison found:**
  - **`CONTEXTRECORD`:** Warpine had 0x88 bytes of floating-point area where IBM's `<bsexcpt.h>` has 0x6C (`ctx_env[7]` and eight packed 10-byte `FPREG`s). Every register was 0x1C bytes past where programs read it, so an exception handler reading `ctx_RegEip`, `ctx_SegCs` or `ctx_RegEsp` got another word. It is now 0xB0 bytes, `ctx_SegGs` at 0x70.
    - `xcpt_thread_test` checks a handler's `CS`, `SS` and `ESP` against the program's own, built by Open Watcom.
  - **`FONTMETRICS`:** Warpine wrote 208 bytes. It now zeroes all 228 (two atoms and a `PANOSE` follow `lMatch`).
- **Sample bugs:**
  - **`socket_test`** and **`pm_frame_accel_test`**: `#pragma pack( __push, n )` is Open Watcom's spelling; GCC and clang ignored it. They use `push` / `pop` now, which Open Watcom builds into the same binaries.
  - **`pm_controls_test`, `pm_demo` and `c_pm_test`** drew strings with a count past their end. The goldens of the controls tests had recorded Open Watcom's `===` from beyond `pm_controls_test`'s title; they are regenerated.
- **Tests:**
  - `lx_link.rs`: 3 for DLLs;
  - `tests/integration.rs`: `sdk_structure_layouts_match_open_watcom`, `sample_path` and the `WARPINE_SAMPLES` runs.
