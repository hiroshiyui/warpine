# ADR-0040: Keep the 16-bit information segments in reserved tiles, refreshed by a host thread

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/infoseg.rs` (`GINFOSEG_ADDR`, `LINFOSEG_ADDR`, `refresh_ginfoseg`), `src/loader/ne_exec.rs`, `src/loader/api_registry.rs`; `samples/infoseg_test`; Open Watcom `h/os21x/bsedos.h` (`GINFOSEG`, `LINFOSEG`, `DosGetInfoSeg`); ADR-0039 (session clock)

## Context

`DosGetInfoSeg` returns two selectors. `selector:0` addresses the global information segment (the time and date, version and scheduler values) and the local one (the process and thread ids). Programs read the time fields directly, often in loops, instead of calling `DosGetDateTime`. Warpine used to write `PIB_BASE >> 4` and `TIB_BASE >> 4`, which are real-mode paragraphs and not selectors, and its NE path was a stub.

## Decision

- **Fixed guest addresses at tile starts.** The `GINFOSEG` lives at `0xA0000` and the `LINFOSEG` at `0xB0000`, each at the start of its own 64 KB tile. The selectors handed out are those tiles' data selectors, as `DosFlatToSel` forms them, so `selector:0` is the segment and `DosSelToFlat` maps it back. The tiles lie above the TIB / PIB pages and below the NE segment base; compile-time asserts check both.
- **A host thread refreshes the time fields.** The first call builds both segments and starts one thread. Every clock tick (32 ms) it rewrites the time and date fields from the session clock (ADR-0039), and it stops when the process ends.
- **`LINFOSEG` describes the calling thread at the time of the call** (`tidCurrent`).
- **Both entry points reach the same code:** the NE dispatcher (16:16 pointers in Pascal order) and the 32-bit registry entry (flat pointers).
- **Values the reference doesn't give are marked unverified in the code:** the time-zone sign (taken as `DATETIME`'s), the session numbers, the priority, and the data-segment sizes.

## Consequences

- 16-bit programs, and 32-bit programs that call the entry directly, get real segments whose clock runs on its own.
- Two guest tiles are permanently reserved, and an executable whose objects reach `0xA0000` would collide with them (as it would already collide with the GDT at `0x80000`).
- **This does not fix the 4OS2 case.** 32-bit programs that call `DOS16GETINFOSEG` through a Far16 thunk, as 4OS2 does, still get nothing: Warpine bypasses Far16 thunks. Such programs also convert selectors with OS/2's tiling formula `(sel >> 3) << 16`, while Warpine's tiled selectors start at GDT index 6. Both are recorded in `doc/TODOs.md`.
