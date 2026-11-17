# ADR-0014: Provide CMD.EXE as a built-in Rust shell

- **Status:** Accepted
- **Date:** 2026-04-01
- **Sources:** commit 0cf6252 (plan); `src/loader/cmd.rs`; `CLAUDE.md` (Builtin CMD.EXE shell)

## Context

Many OS/2 programs start `CMD.EXE` (or `OS2SHELL.EXE`) through `DosExecPgm` to run commands and `.CMD` scripts. IBM's `CMD.EXE` cannot be shipped (ADR-0004).

## Decision

`DosExecPgm` of `CMD.EXE` or `OS2SHELL.EXE` runs Warpine's own shell (`cmd.rs`, `Loader::run_builtin_cmd`): built-in commands (DIR, CD, SET, ECHO, CLS, VER, TYPE, MD, RD, DEL, HELP, EXIT), `.CMD` scripts, `/C` and `/K`, redirection and pipelines, and external programs.

## Consequences

- Programs that shell out work without any IBM component.
- The shell's compatibility is Warpine's responsibility; full-featured shells such as 4OS2 run as ordinary guest programs.
- REXX in `.CMD` files needs a separate bridge (`doc/TODOs.md`).
