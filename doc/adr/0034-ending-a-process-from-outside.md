# ADR-0034: End a process from outside with KILLPROC, its exit list and a grace period

- **Status:** Accepted; extends ADR-0023
- **Date:** 2026-11-18
- **Sources:** batch 6; `src/loader/signals.rs` (`dos_kill_process`, `KillTable`, `request_termination`, `KILL_GRACE`), `src/loader/host_signal.rs`, `src/gui/renderer.rs` / `text_renderer.rs` (`run_pm_loop`, `run_text_loop`); `samples/kill_test`; IBM *Control Program Programming Reference* (`DosKillProcess`, `XCPT_SIGNAL_KILLPROC`, `DosExitList`); ADR-0020, ADR-0023

## Context

On OS/2 a process ended from outside gets `XCPT_SIGNAL_KILLPROC` on thread 1, and its exit-list routines run with `TC_KILLPROCESS`. That happens for `DosKillProcess` from its parent, and for the user closing its window or session. This is how programs flush files and release resources.

Warpine ended such processes at once: the parent killed the host process, and closing a window exited immediately. Exit lists were skipped, and so were endings by end of piped input, faults and KVM errors.

## Decision

- **Requests from other processes are real-time signals** next to ADR-0023's INTR / BREAK:
  - `SIGRTMIN+5` for KILLPROC, `SIGRTMIN+6` for kill-tree;
  - the host's `SIGTERM` maps to KILLPROC and `SIGINT` to INTR;
  - as before, the handler only stores a bit and writes a byte to a pipe.
- **`DosKillProcess` signals a child through a pidfd** taken while the child is known to be unreaped, so neither the request nor a later `SIGKILL` can reach a process that reused the pid.
  - `DKP_PROCESSTREE` makes the child pass the request on to its children first.
  - `DosWaitChild` reports `TC_KILLPROCESS` for a killed child.
- **A grace period:** a process asked to end from outside gets `KILL_GRACE` (5 s) to run its handlers and exit list. Then it ends itself, and the parent sends `SIGKILL` through the pidfd after 7 s. A second close request ends the process at once.
- **Closing a session means KILLPROC plus the exit list**, never an immediate exit: closing the PM desktop, a VIO or text window, or pressing Ctrl+Esc. The GUI loops keep running until the process has ended.
- **Exit lists also run** at the end of piped keyboard input (`TC_EXIT`), after a triple fault or an unhandled VM exit (`TC_TRAP`), and after a KVM error on another thread (`TC_HARDERROR`).

## Consequences

- Programs clean up as on OS/2 when they are killed or closed. A hung program still ends within the grace period.
- Warpine claims two more real-time signals and catches `SIGINT` / `SIGTERM`; a host `timeout` or Ctrl+C now goes through the exit list.
- Without pidfds (Linux before 5.3) a child is killed at once, as before.
- The 5 s grace and the codes reported are Warpine's reading of the reference and are marked unverified.
