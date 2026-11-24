# ADR-0123: Source-level compatibility; Warpine's own ordinals where no public source gives them

- **Status:** Accepted
- **Date:** 2026-11-24
- **Sources:**
  - the maintainer's decision (2026-11-24) that source-level compatibility is enough where ordinals cannot be known;
  - ADR-0006 (Open Watcom's import library as the ordinal source), ADR-0041 (SO32DLL / TCP32DLL ordinals unverified), ADR-0070 (SOM ordinals 1–47 from osFree), ADR-0122 (`somidl`, SOM without IBM's ordinals);
  - IBM's *TCP/IP Programming Reference* (Toolkit 4.5), which says which calls the socket DLLs export but not at which ordinals.

## Context

A built-in module answers calls at ordinals. For most modules a public source gives the real ones: Open Watcom's import libraries (`os2386.lib`, `libconv.lib`, `mmpm2.lib`, `rexx.lib`) and osFree's SOM list. When the two disagree with Warpine, Warpine follows the public source: UCONV and MDM were renumbered on 2026-11-24 for that reason.

Some modules have no public ordinal list. SO32DLL and TCP32DLL (TCP/IP 4.0), and SOM.DLL above ordinal 47, are documented by name only; their ordinals exist in IBM's import libraries and DLLs, which the clean-room policy keeps out of reach. Waiting for a list would leave documented calls out for good.

Warpine's goal is API compatibility, not a clone of OS/2. A program built from source with Warpine's tools (`lx-link`, the SDK, `somidl`) links calls by name through `targets/os2api.def`, whatever number the def gives them.

## Decision

- **Source-level compatibility is the bar where ordinals are unknown.** A documented call may be implemented at an ordinal of Warpine's choosing when no public source gives the real one. Programs built with Warpine's tools reach it by name; `DosQueryProcAddr` finds it by name too.
- **A public ordinal always wins.** Warpine's own numbers are used only where nothing public exists, never against a public source. A Warpine number found later to contradict one is a bug to fix, as UCONV and MDM were.
- **Every such number is marked.**
  - Its def line carries `# NOTE: Warpine's own ordinal (ADR-0123)`, and the registry block says so.
  - Its numbers stay clear of those already used, in a range of their own where the module allows it (SO32DLL and TCP32DLL from 50).
  - The modules are listed in `doc/unverified.md`.

## Consequences

- Documented calls of the socket DLLs and of SOM can be implemented now. The first are the TCP/IP utility routines, `ioctl`, `readv` and `writev` (`tcpip_util.rs`, 2026-11-24).
- A binary built with IBM's import libraries may import such a call at another ordinal and reach the wrong function, or none. Running existing binaries of those modules is not promised; rebuilding from source is.
- If a public ordinal list turns up later, the def, the registry and the tracer change together, and programs built with Warpine's tools need relinking.

## Warpine's choices (unverified)

- The ordinals themselves: SO32DLL 50–52, TCP32DLL 50–71 (`targets/os2api.def`), as well as the earlier SO32DLL / TCP32DLL numbers ADR-0041 already marked unverified.
