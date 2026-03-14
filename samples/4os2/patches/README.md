# 4OS2 Patches for Warpine

These patches modify the upstream 4OS2 source to work with warpine's
OS/2 compatibility layer. They are applied automatically by `fetch_source.sh`.

## Patch List

### 1. bsesub.h — Eliminate 16-bit VIO/KBD thunks

**File:** `h/bsesub.h` (replaces Watcom's `vendor/watcom/h/os2/bsesub.h`)

The Watcom OS/2 headers declare VIO/KBD functions with `APIENTRY16`
(`_Far16 _Pascal`), which makes the compiler generate `__vfthunk`
16-bit bridges for every VIO/KBD call. Warpine cannot execute these
16-bit thunks because the GDT has no 16-bit segment selectors.

Fix: a modified copy of `bsesub.h` with all `APIENTRY16` replaced by
`_System` (32-bit calling convention). This makes VIO/KBD calls go
directly through warpine's 32-bit VIOCALLS/KBDCALLS ordinal dispatch.

### 2. viodirect.h — Additional APIENTRY16 overrides

**File:** `h/viodirect.h` (new file)

Defines `_Seg16` as empty to prevent 16-bit segment pointer types in
`DosGetInfoSeg` and other APIs that use `_Seg16` pointers.

### 3. viowrap.c — 32-bit VIO/KBD import declarations

**File:** `c/viowrap.c` (new file)

Provides `#pragma import` directives for VIO/KBD ordinals, ensuring
the linker resolves them as direct 32-bit imports from VIOCALLS/KBDCALLS
DLLs rather than through the Watcom C runtime's thunk wrappers.

### 4. Makefile — Build configuration

**File:** `Makefile` (replaces upstream build)

Cross-compilation Makefile for Linux using Open Watcom v2:
- Links `os2386.lib` (32-bit) before `os2.lib` (16-bit) so 32-bit
  VIO/KBD imports take precedence
- Includes `viowrap.obj` for import pragmas
- Manual import directives for symbols not in os2386.lib

## Applying Patches

Patches are applied automatically by `fetch_source.sh` after fetching
the upstream source. To re-apply manually:

```bash
cd samples/4os2
cp patches/bsesub.h h/bsesub.h
cp patches/viodirect.h h/viodirect.h
cp patches/viowrap.c c/viowrap.c
```
