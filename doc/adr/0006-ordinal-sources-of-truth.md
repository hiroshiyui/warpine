# ADR-0006: Use the Open Watcom import library and toolkit headers as the sources of ordinals and constants

- **Status:** Accepted
- **Date:** 2026-03-14
- **Sources:** commit 7a3f372 ("correct VIO/KBD ordinals …"); `doc/os2_ordinals.md`; `targets/os2api.def`; `src/bin/gen_api.rs`; `constants_match_toolkit_headers` test; `CLAUDE.md` (Conventions)

## Context

Several API ordinals and constants were wrong in early versions (Windows resource-type numbers, menu-style values, VIO / NLS ordinals), and programs then failed in ways that were hard to trace.

## Decision

- **Ordinals:** `doc/os2_ordinals.md` is a snapshot of Open Watcom's `os2386.lib` import library and is the reference for ordinals. `targets/os2api.def` is the single source Warpine's tools read; `gen_api check` and `gen_api validate-doc` compare it with `api_trace.rs` and with the snapshot, and tests check every PMWIN / PMGPI dispatch arm against the snapshot.
- **Constants:** values that have a toolkit `#define` keep the toolkit's name and value, copied from the headers, and the `constants_match_toolkit_headers` test compares them. No magic numbers: named constants only.
- The ordinal snapshot is not extended from Warpine's own assignments, only from authoritative SDK sources.

## Consequences

- A wrong ordinal or constant fails a test instead of a program.
- Open Watcom's headers and library are a build and test dependency (`vendor/setup_watcom.sh`).
