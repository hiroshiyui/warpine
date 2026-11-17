# ADR-0017: Store OS/2 profiles (INI files) in Warpine's own text format

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** commit 234ff2a ("PMSHAPI profiles and HELPMGR help instances"); `src/loader/profile.rs`; reference manual (PMSHAPI)

## Context

PM programs keep their settings in profiles through the `Prf*` calls: OS2.INI, OS2SYS.INI and private INI files. IBM does not publicly document the binary INI layout, so reproducing it would break the clean-room policy (ADR-0004).

## Decision

Profiles are stored in a text format of Warpine's own: a `; warpine-profile 1` line, `["Application"]` sections and `"Key" = "value"` lines, with names and values quoted and escaped so any byte sequence round-trips. OS2.INI and OS2SYS.INI live in `C:\OS2`; other profiles at the path the program gives, resolved like `DosOpen`. Every change is written atomically (temporary file, then rename) through the VFS.

## Consequences

- Profiles are readable and editable with a text editor and safe to diff.
- Binary INI files shipped with OS/2 programs cannot be read; they are refused as corrupt until a public description of the format allows an importer.
