# ADR-0094: Warpine's structures in a system area, programs checked against their area

- **Status:** Accepted; amends ADR-0088 (where `TLM_PAGE` is) and ADR-0040 (where the information segments are)
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Guide*: a 32-bit program's private memory starts at 64 KB, where linkers place EXE objects; `DosGetInfoBlocks`.
  - The LX format's object table: base address, virtual size, page count.
  - Intel SDM vol. 3, chapter 4: 32-bit paging with 4 MB pages.
  - The code: `src/loader/constants.rs`, `paging.rs`, `infoseg.rs`, `lx_loader.rs` (`check_exe_objects`, `load`).
  - The samples: `samples/bigdata_test`; `samples/npipe_cut_test`, where the fault showed (ADR-0093).

## Context

Warpine kept its own structures at fixed addresses from 0x80000 to 0xBFFFF:
- the GDT (0x80000) and the IDT and its handlers (0x8D000);
- thread 1's TIB (0x90000), the PIB (0x91000) and the environment page;
- the thread-local page (0x9F000, ADR-0088);
- the information segments (0xA0000 and 0xB0000).

A comment said "LX objects typically end below 0x80000". Open Watcom links EXE objects from 64 KB on, so any program with more than about 450 KB of static data has objects that reach this band:
- the loader copied the program's pages over Warpine's structures;
- Warpine then wrote over the program's data;
- inside the thread-local page, each thread saw its own page instead.

`npipe_cut_test`'s two 512 KB arrays read back zeros there. The loader checked nothing: an object outside guest memory panicked it, and one over the API thunks or the allocator's memory was loaded anyway.

The rule "TIB and PIB below 1 MB, for 16-bit segment arithmetic" no longer held: 16-bit code reaches them through the tiled selectors, which cover all 256 MB, and nothing computes real-mode segments.

## Decision

1. **A system area** at `SYSTEM_AREA_BASE` = 0x01800000, 256 KB, between the API thunks (16 MB) and the allocator (32 MB). In order:
   - the GDT, IDT and IDT handlers;
   - thread 1's TIB, the PIB and the environment page;
   - the thread-local page (+0x1F000);
   - the `GINFOSEG` and `LINFOSEG`, a 64 KB tile each.

   `const` assertions keep it clear of the thunks, the allocator and the program area.
2. **Paging.** The page directory sends the 4 MB block holding `TLM_PAGE` (`TLM_CHUNK`) through the page table; every other 4 MB is a large page. The rest of ADR-0088 is unchanged.
3. **A program area.** `EXE_AREA_START` (64 KB) to `EXE_AREA_END` (the API thunks, 16 MB). `check_exe_objects` refuses an EXE whose objects leave it, counting each object's pages, or overlap one another. Loading fails with an error instead of a panic, also for a page outside guest memory.
4. **Resource objects** (`OBJRSRC`, flag 0x0008) are reached only through the resource table, never by fixups, and linkers give them base address 0. They are not checked against the program area: the allocator places them, at most 64 MB each. Before, they were copied to their base address, page 0.
5. **DLLs and the 16-bit (NE) path are unchanged.** DLL objects come from the allocator, and NE segments start at 1 MB.

## Warpine's choices (unverified)

- **Program area size:** 16 MB, so an EXE whose objects reach past it is refused. OS/2 allows a larger private arena.
- **Rejected objects:** objects that overlap, or that start below 64 KB, are refused rather than loaded anyway.
- **Resource objects** go wherever the allocator puts them, whatever their base address says.
- **Where things live:** the system area's address, and the information segments' place in it.

## Consequences

- **Large programs work:** a program owns all of its static data, however large, up to 16 MB of objects.
- **Malformed EXEs** are refused with a message instead of crashing Warpine or overwriting its structures.
- **Below 1 MB,** nothing of Warpine's remains in a 32-bit process.
- **Future fixed structures** go into the system area, with an assertion.
- **Tests:**
  - `paging.rs`: the tables for the moved page;
  - `lx_loader.rs` `exe_objects_must_fit_the_program_area`;
  - `tests/integration.rs` `test_large_static_data_is_the_programs_own`: `bigdata_test`, 13 checks with 1.5 MB of static data, covering its own writes, the TIB and PIB outside it, a thread, thread-local memory, `DosGetInfoSeg`, and file I/O across all of it;
  - every existing sample, 32-bit and 16-bit, on the new layout; the PM samples with resources cover resource objects;
  - `thunk_test` now checks that the TIB and PIB lie outside the program area, where it used to require Warpine's old addresses.
