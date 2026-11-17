# ADR-0004: Implement OS/2 from public documentation only (near-clean-room)

- **Status:** Accepted
- **Date:** 2026-03-18
- **Sources:** commit f42e7b0 ("establish near-clean-room engineering policy in CLAUDE.md"); `CLAUDE.md` (Engineering policy); `doc/TODOs.md` (Engineering Policy)

## Context

OS/2 is IBM's proprietary system. Using its binaries, ROMs or disassembled code would create legal risk and could taint Warpine's code.

## Decision

Warpine's API behaviour is derived from public documentation only: IBM's programming references and the Developer Connection material (read through the EDM2 wiki), the OS/2 toolkit headers (Open Watcom's `h/os2`), and osFree / ReactOS references. Warpine uses no IBM DLL binaries, no ROM dumps, and no disassembly of OS/2 system libraries.

- The primary test oracle is the observable behaviour of programs built with Open Watcom (ADR-0018).
- Where the public sources do not settle a behaviour, Warpine chooses one and records it as "unverified" in `doc/TODOs.md` and in the module's documentation.

## Consequences

- Some behaviour is Warpine's own choice until a public source confirms it; the unverified lists show where.
- Formats that IBM never documented are not reproduced byte for byte. Warpine uses its own format instead, for example for profiles (ADR-0017).
- Contributors must not add code or data derived from OS/2 binaries.
