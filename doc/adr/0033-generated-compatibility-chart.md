# ADR-0033: Generate the API compatibility chart from Warpine's source and a dated, cited API table

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/bin/gen_compat.rs`; `doc/compat/os2_api_versions.tsv`, `doc/compat/SOURCES.md`, `doc/compat/excluded.tsv`, `doc/compat/extra_modules.tsv`; the maintainer's request for a bar chart of API compatibility with OS/2 2.0, 3.0 and 4.0 at the head of the README; ADR-0004, ADR-0006, ADR-0021

## Context

The project wants to show how much of the OS/2 API Warpine covers, compared with OS/2 2.0, Warp 3 and Warp 4, at the head of the README. A hand-made picture would go stale with every API added, and a percentage needs an honest denominator: which calls each version had.

Warpine's only ordinal source, the Open Watcom import library snapshot (`doc/os2_ordinals.md`, ADR-0006), lists the exports of one late toolkit and says nothing about when each call appeared.

## Decision

- **A dated API table, committed with its sources.**
  - `doc/compat/os2_api_versions.tsv` gives every 32-bit export of `os2386.lib` the version it first appeared in: 2.0 (2.1 / 2.11 folded in), 3.0, 4.0, `later` or `unknown`.
  - The dating comes from public documentation only (ADR-0004): the IBM 2.0 and Warp 3 references, the Warp 3 Debugging Handbook's DOSCALLS table, the Toolkit 4.5 books and EDM/2.
  - Each line names its source. Lines dated only by the ordinal-chronology rule are marked `H1`.
  - `SOURCES.md` documents the method, the confidence per module and the weakest parts.
  - 16-bit entry points and Workplace Shell class-data exports are not counted; `excluded.tsv` lists the dropped lines.
- **What counts as implemented is read from the source.**
  - `gen_compat` takes the registry entries (module and ordinal) and the PM dispatch arms (ordinal and name, the form the ordinal tests already enforce).
  - Stubs (`STUB_ORDINALS`, arms marked "(stub)") do not count.
  - Calls dated `later` or `unknown` count in no version.
- **The chart:**
  - grouped horizontal bars per subsystem and overall, one bar per version;
  - every bar labelled with its percentage and count;
  - a light and a dark SVG, coloured from the validated reference palette and embedded with `<picture>` at the head of the README;
  - `gen_compat table` prints the same numbers as text.
- **Drift is a test failure.** A unit test fails when the committed SVGs differ from what the table and the source give, so a change that adds or removes an API must re-render the chart (`cargo run --bin gen_compat -- render`).

## Consequences

- The chart is always current and reproducible, and its denominator can be checked line by line against cited documents.
- Every API change touches two generated files. That is the price of a chart that cannot go stale.
- The measure counts entry points, not behaviour: an implemented call may still be partial, and a whole subsystem counts the same per call however much it matters.
- **What the chart does not cover:**
  - VIO / KBD / MOU (16-bit entry points in the library) and DLLs outside `os2386.lib` (UCONV, MMPM/2, TCP/IP, PMCTLS, MSG); their partial lists are in `extra_modules.tsv`.
  - The Warp 4 bar understates Warp 4's additions, which are mostly in such DLLs.
- Better dating, especially of GPI after 2.0 and of Warp 3's DOSCALLS, changes only the TSV; the chart follows on the next render.
