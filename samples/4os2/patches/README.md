# 4OS2 Patches for Warpine

These patches modify the upstream 4OS2 source to work with warpine's
OS/2 compatibility layer. They are applied automatically by `fetch_source.sh`.

## Patch List

### 1. bsesub.h.patch — Eliminate 16-bit VIO/KBD thunks

**Type:** unified diff against `vendor/watcom/h/os2/bsesub.h`
**Applied to:** `h/bsesub.h`

The Watcom OS/2 headers declare VIO/KBD functions with `APIENTRY16`
(`_Far16 _Pascal`), which makes the compiler generate `__vfthunk`
16-bit bridges for every VIO/KBD call. Warpine cannot execute these
16-bit thunks because the GDT has no 16-bit code segment selectors.

**Change:** All `APIENTRY16` replaced with `_System` (32-bit calling
convention). This makes VIO/KBD calls go directly through warpine's
32-bit VIOCALLS/KBDCALLS ordinal dispatch without 16-bit thunking.

### 2. viodirect.h — APIENTRY16 / _Seg16 overrides (new file)

**Type:** new file
**Applied to:** `h/viodirect.h`

Overrides `APIENTRY16` and `_Seg16` macros after os2def.h has defined
them, as an additional safety net for any code that includes os2def.h
directly instead of through bsesub.h.

### 3. viowrap.c — 32-bit VIO/KBD import declarations (new file)

**Type:** new file
**Applied to:** `c/viowrap.c`

Provides `#pragma import` directives for all VIO/KBD ordinals used by
4OS2, ensuring the linker resolves them as direct 32-bit imports from
VIOCALLS/KBDCALLS instead of through the Watcom C runtime's thunk
wrappers in `clib3r.lib`.

## Applying Patches

Patches are applied automatically by `fetch_source.sh` after fetching
the upstream source. To apply manually:

```bash
cd samples/4os2
# Apply bsesub.h diff patch
cp ../../vendor/watcom/h/os2/bsesub.h h/bsesub.h
patch -p2 h/bsesub.h < patches/bsesub.h.patch
# Copy new files
cp patches/viodirect.h h/viodirect.h
cp patches/viowrap.c c/viowrap.c
```
