# ADR-0036: Give every process a session-unique id chosen by its parent; follow the documented entry ABI

- **Status:** Accepted; supersedes the process-id consequence of ADR-0020
- **Date:** 2026-11-18
- **Sources:** batch 6; `src/loader/inherit.rs` (`admit_child`, `Admission`, `WARPINE_PID` / `WARPINE_PPID`, `write_pib_process_ids`), `src/loader/session_threads.rs` (`pid-next`), `src/loader/vcpu.rs` (`exe_entry_stack`, the start-up check); `vendor/setup_watcom.sh`; `samples/argv_test`; Open Watcom v2 C runtime source (`cstrto32.asm`, `osmain.h`, `maino32.c`) and its fix #1649 (2026-08-11); ADR-0020, ADR-0032

## Context

ADR-0020 noted that every Warpine process reported the constant id 42 for itself, while a parent numbered its children from 100. A child's own id therefore differed from the one its parent used with `DosWaitChild`, `DosKillProcess` and signals.

Separately, Open Watcom-built samples had an empty `argv` and no environment. The TODO list blamed Warpine's entry stack, but Warpine already followed the documented layout: `[esp+4]` hmod, `[esp+8]` 0, `[esp+12]` environment, `[esp+16]` command line. The vendored Open Watcom snapshot of March 2026 read every entry argument one slot off, a bug fixed upstream on 2026-08-11. Programs built with it fail on real OS/2 too.

## Decision

- **The parent chooses each child's id** when it admits the child (`admit_child` returns an `Admission` with the reserved thread slot and the id) and passes it as `WARPINE_PID`, with its own as `WARPINE_PPID`.
  - Ids come from a counter file `pid-next` in the ADR-0032 session directory, under its `flock`: 100 to 0xFFFF, then wrapping to 100, parsed strictly.
  - The first process is 42, with parent 0.
  - Without a usable directory, ids are numbered per process.
- **Every user of process ids agrees:** `pib_ulpid` / `pib_ulppid`, `RESULTCODES.codeTerminate` of `EXEC_ASYNC`, `DosWaitChild`, `DosKillProcess`, `DosSendSignalException`, `DosQueryMutexSem`, `DosGiveSharedMem`, `WinQueryWindowProcess` and the switch list.
- **Warpine implements the documented entry ABI and does not emulate toolchain bugs.**
  - It recognises the broken Open Watcom start-up code and logs a warning.
  - `vendor/setup_watcom.sh` warns about an old snapshot and gains `--refresh` (download and unpack beside the old one, swap only on success).
  - The samples are built with a fixed snapshot.

## Consequences

- A child and its parent agree on ids, and ids are unique within a session (not across sessions).
- Programs built with a pre-August-2026 Open Watcom get an empty `argv` and environment; the warning says so.
- Samples with a child mode still read `pib_pchcmd`, so a toolchain regression cannot make a sample start itself without end (ADR-0028).
- Gaps:
  - Exit codes reach the parent through the host process, so only 8 bits arrive.
  - Signals to processes other than one's own children need an id-to-host-pid map for the session.
