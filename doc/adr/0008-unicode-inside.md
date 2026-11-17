# ADR-0008: Keep text as Unicode inside Warpine and convert at the codepage boundary

- **Status:** Accepted
- **Date:** 2026-03-17
- **Sources:** commit a4c75ca ("add Unicode-internal architecture as a future goal"); `src/loader/codepage.rs`; `CLAUDE.md` (Status: Unicode-internal architecture)

## Context

OS/2 programs pass text in a codepage: the process codepage, a message queue's (`WinSetCp`) or a presentation space's (`GpiSetCp`); SBCS and DBCS. The host, fonts and clipboard work in Unicode.

## Decision

Text is decoded to Unicode where it enters Warpine (`cp_decode`, `read_guest_string`) and encoded back where it is returned to the guest (`cp_encode`), using embedded tables (CP437 / 850 / 852, CP1004) and `encoding_rs` for the rest. The VIO buffer stores Unicode cells and keeps the original bytes for DBCS cell annotation and read-back.

The codepage that applies is the narrowest one in effect: the presentation space's, the window queue's, then the process's.

## Consequences

- Rendering, fonts, clipboard and host file names see Unicode only.
- Every guest-text entry and exit point must name its codepage; a missed conversion shows as mojibake, so conversions are covered by tests.
